//! Engine frame-rate places found in main at build time, without a build ID (nxnative's `unity_fps.py`,
//! `ue_cvar.py`, `ue_fixed_rate.py`, `monolith_fps.py`, their designs reimplemented). `nxrs-build` writes what it finds
//! to `<data>\fps_lock.txt`; os `fpslock` writes the profile's `fps_target` there at each present.
//!
//! * Unity: Application.set_targetFrameRate's global and QualitySettings.get_vSyncCount's walk (the global, the
//!   level index, the levels, a level's size, the count), from IL2CPP's internal-call table (names then functions)
//!   or from the calls that register them in code (Unity 2019.4 / 2017.4).
//! * Unreal: the TAutoConsoleVariables registered as t.MaxFPS, rhi.SyncInterval, nvn.SyncInterval,
//!   Project.FrameRateLimit (value pointer at +0x10); UEngine::FixedFrameRate on GEngine and the bUseFixedFrameRate bit.
//! * Monolith: the frame interval (an int32 of vsyncs, 0 = 2) through its reviewed wait / duration / setter functions.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use translator::aot::nso_match::{writes_gpr, Image};

const RET: u32 = 0xD65F_03C0;

fn word(img: &[u8], a: usize) -> u32 {
    img.get(a..a + 4).map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap()))
}
fn adrp_page(w: u32, pc: usize) -> i64 {
    let mut imm = (((((w >> 5) & 0x7FFFF) << 2) | ((w >> 29) & 3)) as i64) << 12;
    if imm & (1 << 32) != 0 {
        imm -= 1 << 33;
    }
    (pc as i64 & !0xFFF) + imm
}
fn is_adrp(w: u32) -> bool {
    w & 0x9F00_0000 == 0x9000_0000
}
fn branch(w: u32, pc: usize) -> i64 {
    let off = ((w & 0x3FF_FFFF) << 2) as i64;
    pc as i64 + if off & (1 << 27) != 0 { off - (1 << 28) } else { off }
}
fn find_bytes(img: &[u8], pat: &[u8], from: usize, to: usize) -> Vec<usize> {
    let to = to.min(img.len());
    if from >= to || pat.is_empty() {
        return vec![];
    }
    img[from..to].windows(pat.len()).enumerate().filter(|(_, w)| *w == pat).map(|(i, _)| from + i).collect()
}
fn cstr(img: &[u8], v: usize) -> Option<&[u8]> {
    let e = img.get(v..(v + 256).min(img.len()))?.iter().position(|&b| b == 0)?;
    Some(&img[v..v + e])
}

/// The module as the finders read it: the image (with bss zeros), its segments, its relative relocations
/// ({slot: target}, module offsets).
pub struct Main<'a> {
    pub img: &'a [u8],
    pub text_end: usize,
    pub ro_off: usize,
    pub data_off: usize,
    pub rel: HashMap<u64, u64>,
}

impl<'a> Main<'a> {
    pub fn of(nso: &'a translator::loader::nso::Nso) -> Main<'a> {
        let rel = nso.relocs.iter().filter(|r| r.typ == 1027).map(|r| (r.off, r.addend as u64)).collect();
        Main { img: &nso.image, text_end: nso.text.0 + nso.text.1, ro_off: nso.rodata.0, data_off: nso.data.0, rel }
    }
    fn w(&self, a: usize) -> u32 {
        word(self.img, a)
    }
}

// ---------------- Unity

pub const TFR: &str = "UnityEngine.Application::set_targetFrameRate";
pub const VSYNC: &str = "UnityEngine.QualitySettings::get_vSyncCount";

#[derive(Debug, Default, PartialEq)]
pub struct Unity {
    pub target: Option<u64>,
    /// (QualitySettings global, level index offset, levels offset, level size, count offset)
    pub qs: Option<(u64, u32, u32, u32, u32)>,
}

fn icall_table(m: &Main) -> BTreeMap<String, u64> {
    let name_at = |v: u64| -> Option<String> {
        let v = v as usize;
        if !(m.ro_off <= v && v < m.data_off) {
            return None;
        }
        let s = cstr(m.img, v)?;
        (s.windows(2).any(|w| w == b"::") && s.is_ascii()).then(|| String::from_utf8_lossy(s).into_owned())
    };
    let mut pat = vec![0u8];
    pat.extend(TFR.as_bytes());
    pat.push(0);
    let Some(&key) = find_bytes(m.img, &pat, 0, m.img.len()).first() else { return BTreeMap::new() };
    let Some(&slot) = m.rel.iter().filter(|(_, &v)| v == key as u64 + 1).map(|(s, _)| s).min() else { return BTreeMap::new() };
    let (mut start, mut end) = (slot, slot);
    while m.rel.get(&(start.wrapping_sub(8))).and_then(|&v| name_at(v)).is_some() {
        start -= 8;
    }
    while m.rel.get(&end).and_then(|&v| name_at(v)).is_some() {
        end += 8;
    }
    let n = (end - start) / 8;
    if !(0..n).all(|i| m.rel.get(&(end + 8 * i)).is_some_and(|&f| f > 0 && (f as usize) < m.ro_off)) {
        return BTreeMap::new();
    }
    (0..n).filter_map(|i| Some((name_at(m.rel[&(start + 8 * i)])?, m.rel[&(end + 8 * i)]))).collect()
}

fn icall_code(m: &Main, names: &[&str]) -> BTreeMap<String, u64> {
    let mut out = BTreeMap::new();
    for name in names {
        let mut pat = vec![0u8];
        pat.extend(name.as_bytes());
        pat.push(0);
        let Some(s) = find_bytes(m.img, &pat, m.ro_off, m.data_off).first().map(|s| s + 1) else { continue };
        let (lo, page) = ((s & 0xFFF) as u32, (s & !0xFFF) as i64);
        let add = 0x9100_0000 | lo << 10; // add x0, x0, #lo
        let mut fns = BTreeSet::new();
        let mut j = 16;
        while j + 8 <= m.text_end {
            if m.w(j) == add {
                let a = m.w(j - 4);
                if is_adrp(a) && a & 31 == 0 && adrp_page(a, j - 4) == page && m.w(j + 4) & 0xFC00_0000 == 0x9400_0000 {
                    let (p, l) = (m.w(j - 12), m.w(j - 8));
                    if is_adrp(p) && p & 31 == 1 {
                        let pg = adrp_page(p, j - 12);
                        let f = if l & 0xFFC0_03FF == 0xF940_0021 {
                            m.rel.get(&((pg + ((l >> 10) & 0xFFF) as i64 * 8) as u64)).copied()
                        } else if l & 0xFFC0_03FF == 0x9100_0021 {
                            Some((pg + ((l >> 10) & 0xFFF) as i64) as u64)
                        } else {
                            None
                        };
                        if let Some(f) = f.filter(|&f| (f as usize) < m.text_end) {
                            fns.insert(f);
                        }
                    }
                }
            }
            j += 4;
        }
        if fns.len() == 1 {
            out.insert(name.to_string(), *fns.iter().next().unwrap());
        }
    }
    out
}

fn follow(m: &Main, mut f: u64) -> Option<u64> {
    for _ in 0..2 {
        let u = m.w(f as usize);
        if u & 0xFC00_0000 != 0x1400_0000 {
            break;
        }
        let t = branch(u, f as usize);
        if !(0..m.text_end as i64).contains(&t) {
            return None;
        }
        f = t as u64;
    }
    Some(f)
}

fn target_fps(m: &Main, f: u64) -> Option<u64> {
    let f = follow(m, f)? as usize;
    let mut page: HashMap<u32, i64> = HashMap::new();
    for i in 0..8 {
        let a = f + 4 * i;
        let u = m.w(a);
        if is_adrp(u) {
            page.insert(u & 31, adrp_page(u, a));
            continue;
        }
        if u & 0xFFC0_0000 == 0xB900_0000 && u & 31 == 0 {
            let n = (u >> 5) & 31;
            return page.get(&n).map(|p| (p + ((u >> 10) & 0xFFF) as i64 * 4) as u64);
        }
        if u == RET {
            return None;
        }
    }
    None
}

fn context_global(m: &Main, mut f: usize) -> Option<u64> {
    let framed = m.w(f) == 0xA9BF_7BFD && m.w(f + 4) == 0x9100_03FD && m.w(f + 16) == 0xA8C1_7BFD && m.w(f + 20) == RET;
    if framed {
        f += 8;
    }
    let u = m.w(f);
    if u & 0xFFE0_001F != 0x5280_0000 {
        return None;
    }
    let v = m.w(f + 4);
    if v & 0xFC00_0000 != if framed { 0x9400_0000 } else { 0x1400_0000 } {
        return None;
    }
    let h = branch(v, f + 4);
    if !(0..m.text_end as i64 - 12).contains(&h) {
        return None;
    }
    let h = h as usize;
    let (a, b, c, d) = (m.w(h), m.w(h + 4), m.w(h + 8), m.w(h + 12));
    if !is_adrp(a) || b & 0xFFC0_0000 != 0x9100_0000 || (b >> 5) & 31 != a & 31 {
        return None;
    }
    let (base, r) = (adrp_page(a, h) + ((b >> 10) & 0xFFF) as i64, b & 31);
    let k = 8 * ((u >> 5) & 0xFFFF) as i64;
    if c & 0xFFE0_FC00 == 0xF860_D800 && (c >> 5) & 31 == r && (c >> 16) & 31 == 0 {
        return Some((base + k) as u64);
    }
    if c & 0xFFE0_FC00 == 0x8B20_CC00 && (c >> 16) & 31 == 0 && c & 31 == r && (c >> 5) & 31 == r && d & 0xFFC0_03FF == 0xF940_0000 | r << 5 {
        return Some((base + ((d >> 10) & 0xFFF) as i64 * 8 + k) as u64);
    }
    None
}

#[derive(Clone, Copy, PartialEq)]
enum Val {
    Page(i64),
    Q,
    Idx,
    Arr,
    Imm(u32),
    Lvl(u32),
}

fn vsync_count(m: &Main, f: u64) -> Option<(u64, u32, u32, u32, u32)> {
    let f = follow(m, f)? as usize;
    let mut regs: HashMap<u32, Val> = HashMap::new();
    let (mut q, mut idx, mut arr): (Option<u64>, Option<u32>, Option<u32>) = (None, None, None);
    for i in 0..24 {
        let a = f + 4 * i;
        let u = m.w(a);
        let (d, n, mm) = (u & 31, (u >> 5) & 31, (u >> 16) & 31);
        if u == RET {
            return None;
        }
        if is_adrp(u) {
            regs.insert(d, Val::Page(adrp_page(u, a)));
        } else if u & 0xFC00_0000 == 0x9400_0000 {
            let t = branch(u, a);
            q = if (0..m.text_end as i64 - 8).contains(&t) { context_global(m, t as usize) } else { None };
            q?;
            regs = HashMap::from([(0, Val::Q)]);
        } else if u & 0xFFC0_0000 == 0xF940_0000 {
            let imm = ((u >> 10) & 0xFFF) * 8;
            match regs.get(&n).copied() {
                Some(Val::Page(p)) if q.is_none() => {
                    q = Some((p + imm as i64) as u64);
                    regs.insert(d, Val::Q);
                }
                Some(Val::Q) => {
                    arr = Some(imm);
                    regs.insert(d, Val::Arr);
                }
                _ => {
                    regs.remove(&d);
                }
            }
        } else if u & 0xFFC0_0000 == 0xB980_0000 {
            if regs.get(&n) == Some(&Val::Q) {
                idx = Some(((u >> 10) & 0xFFF) * 4);
                regs.insert(d, Val::Idx);
            } else {
                regs.remove(&d);
            }
        } else if u & 0x7FE0_0000 == 0x5280_0000 {
            regs.insert(d, Val::Imm((u >> 5) & 0xFFFF));
        } else if u & 0xFFE0_0000 == 0x8B00_0000 && (u >> 22) & 3 == 0 {
            if regs.get(&n) == Some(&Val::Arr) && regs.get(&mm) == Some(&Val::Idx) {
                regs.insert(d, Val::Lvl(1 << ((u >> 10) & 63)));
            } else {
                regs.remove(&d);
            }
        } else if u & 0xFFE0_8000 == 0x9B00_0000 {
            match (regs.get(&n), regs.get(&mm), regs.get(&((u >> 10) & 31))) {
                (Some(Val::Idx), Some(Val::Imm(s)), Some(Val::Arr)) => {
                    let s = *s;
                    regs.insert(d, Val::Lvl(s));
                }
                _ => {
                    regs.remove(&d);
                }
            }
        } else if u & 0xFFC0_0000 == 0xB940_0000 {
            return match (regs.get(&n), q, idx, arr) {
                (Some(Val::Lvl(size)), Some(q), Some(idx), Some(arr)) if d == 0 => Some((q, idx, arr, *size, ((u >> 10) & 0xFFF) * 4)),
                _ => None,
            };
        } else if u & 0x0A00_0000 != 0x0800_0000 || (u >> 22) & 1 != 0 {
            regs.remove(&d);
        }
    }
    None
}

pub fn unity(m: &Main) -> (Unity, String) {
    let mut calls = icall_table(m);
    if calls.is_empty() {
        calls = icall_code(m, &[TFR, VSYNC]);
    }
    if calls.is_empty() {
        return (Unity::default(), "no Unity internal-call table".into());
    }
    let (mut out, mut why) = (Unity::default(), vec![]);
    if let Some(&f) = calls.get(TFR) {
        match target_fps(m, f).filter(|&g| g >= m.data_off as u64) {
            Some(g) => {
                out.target = Some(g);
                why.push(format!("targetFrameRate at {g:#x}"));
            }
            None => why.push(format!("[RED] targetFrameRate setter at {f:#x} not read")),
        }
    }
    if let Some(&f) = calls.get(VSYNC) {
        match vsync_count(m, f).filter(|v| v.0 >= m.data_off as u64) {
            Some(v) => {
                out.qs = Some(v);
                why.push(format!("vSyncCount at [[{:#x}]+{:#x}] + [[{:#x}]+{:#x}] * {:#x} + {:#x}", v.0, v.2, v.0, v.1, v.3, v.4));
            }
            None => why.push(format!("[RED] vSyncCount getter at {f:#x} not read")),
        }
    }
    (out, format!("Unity internal calls ({}): {}", calls.len(), why.join(", ")))
}

// ---------------- Unreal console variables

pub const UE_NAMES: [&str; 4] = ["t.MaxFPS", "rhi.SyncInterval", "nvn.SyncInterval", "Project.FrameRateLimit"];

fn add_imm(w: u32) -> Option<(u32, u32, u64)> {
    (w & 0xFF80_0000 == 0x9100_0000).then(|| (w & 31, (w >> 5) & 31, (((w >> 10) & 0xFFF) as u64) << (12 * ((w >> 22) & 1))))
}

/// Whether u may write general register r (ue_cvar.py `writes`: errs towards yes).
fn ue_writes(u: u32, r: u32) -> bool {
    let (d, n, t2) = (u & 31, (u >> 5) & 31, (u >> 10) & 31);
    if u & 0x7C00_0000 == 0x1400_0000 {
        return u >> 31 != 0 && r == 30;
    }
    if u & 0x1C00_0000 == 0x1000_0000 || u & 0x0E00_0000 == 0x0A00_0000 {
        return d == r;
    }
    if u & 0x0A00_0000 == 0x0800_0000 {
        if u & 0x3F00_0000 == 0x0800_0000 {
            return r == d || r == t2 || r == (u >> 16) & 31;
        }
        if u & 0x3A00_0000 == 0x2800_0000 {
            return ((u >> 22) & 1 != 0 && (u >> 26) & 1 == 0 && (r == d || r == t2)) || (matches!((u >> 23) & 3, 1 | 3) && r == n);
        }
        if u & 0x3B00_0000 == 0x1800_0000 {
            return d == r;
        }
        if u & 0x3B00_0000 == 0x3800_0000 {
            if (u >> 21) & 1 != 0 && (u >> 10) & 3 == 0 {
                return d == r;
            }
            if (u >> 21) & 1 == 0 && (u >> 10) & 1 != 0 && r == n {
                return true;
            }
            return (u >> 22) & 3 != 0 && d == r && (u >> 26) & 1 == 0;
        }
        if u & 0x3B00_0000 == 0x3900_0000 {
            return (u >> 22) & 3 != 0 && d == r && (u >> 26) & 1 == 0;
        }
        return false;
    }
    if u & 0x0E00_0000 == 0x0E00_0000 {
        return u & 0x5F20_FC00 == 0x1E20_0000 && d == r;
    }
    if u & 0xFFF0_0000 == 0xD530_0000 {
        return d == r;
    }
    false
}

fn branch_target(u: u32, s: i64) -> Option<i64> {
    let sx = |v: u32, bits: u32| -> i64 { if v >> (bits - 1) != 0 { v as i64 - (1i64 << bits) } else { v as i64 } };
    if u & 0x7C00_0000 == 0x1400_0000 {
        return Some(s + 4 * sx(u & 0x3FF_FFFF, 26));
    }
    if u & 0xFF00_0010 == 0x5400_0000 || u & 0x7E00_0000 == 0x3400_0000 {
        return Some(s + 4 * sx((u >> 5) & 0x7FFFF, 19));
    }
    if u & 0x7E00_0000 == 0x3600_0000 {
        return Some(s + 4 * sx((u >> 5) & 0x3FFF, 14));
    }
    None
}

fn base_of(m: &Main, r: u32, t: i64) -> Option<u64> {
    let mut lo: Option<u64> = None;
    let mut found = None;
    let mut s = t - 4;
    while s > (t - 4 * 4096).max(-4) {
        let u = m.w(s as usize);
        if u == RET || u & 0xFFFF_FC1F == 0xD61F_0000 {
            return None;
        }
        if u & 0xFC00_0000 == 0x1400_0000 {
            match branch_target(u, s) {
                Some(tg) if s < tg && tg <= t => {}
                _ => return None,
            }
        }
        if r < 19 && (u & 0xFC00_0000 == 0x9400_0000 || u & 0xFFFF_FC1F == 0xD63F_0000) {
            return None;
        }
        if ue_writes(u, r) {
            if is_adrp(u) && u & 31 == r {
                found = Some((s, u));
                break;
            }
            match add_imm(u) {
                Some((d, n, imm)) if lo.is_none() && d == r && n == r => lo = Some(imm),
                _ => return None,
            }
        }
        s -= 4;
    }
    let (s, u) = found?;
    let mut b = (s - 0x10_0000).max(0);
    while b < (m.text_end as i64).min(t + 0x10_0000) {
        if !(s <= b && b < t) {
            if let Some(x) = branch_target(m.w(b as usize), b) {
                if s < x && x <= t {
                    return None;
                }
            }
        }
        b += 4;
    }
    Some((adrp_page(u, s as usize) + lo.unwrap_or(0) as i64) as u64)
}

fn registered_at(m: &Main, j: usize) -> Option<u64> {
    let na = add_imm(m.w(j))?;
    if na.0 != 1 {
        return None;
    }
    let mut val: HashMap<u32, u64> = HashMap::new();
    let (mut name, mut registered) = (false, false);
    let mut stored: BTreeSet<(u32, u32)> = BTreeSet::new();
    let mut after = 0;
    let mut t = j.saturating_sub(64);
    while t < m.text_end.min(j + 192) {
        let u = m.w(t);
        let call = u & 0xFC00_0000 == 0x9400_0000 || u & 0xFFFF_FC1F == 0xD63F_0000;
        if t < j && (call || u & 0xFC00_0000 == 0x1400_0000 || u == RET) {
            val.clear();
            t += 4;
            continue;
        }
        if call {
            if !registered {
                if !name {
                    return None;
                }
                if u >> 26 == 0x25 {
                    return val.get(&0).copied();
                }
                registered = true;
            } else {
                after += 1;
            }
            for r in 0..19 {
                val.remove(&r);
            }
            stored.retain(|&(n, _)| n >= 19);
            name = false;
            t += 4;
            continue;
        }
        if u & 0xFC00_0000 == 0x1400_0000 || u == RET {
            break;
        }
        if registered && u & 0xFFC0_0000 == 0xF900_0000 {
            let (n, k) = ((u >> 5) & 31, ((u >> 10) & 0xFFF) * 8);
            if u & 31 == 0 && k == 0x10 {
                if let Some(&v) = val.get(&n) {
                    return Some(v);
                }
            }
            if u & 31 == 0 && after > 0 && !val.contains_key(&n) && k >= 0x10 && stored.contains(&(n, k - 0x10)) {
                return base_of(m, n, t as i64).map(|b| b + k as u64 - 0x10);
            }
            stored.insert((n, k));
            t += 4;
            continue;
        }
        let d = u & 31;
        let mut v: Option<u64> = None;
        if is_adrp(u) {
            if registered && d == 1 {
                break;
            }
            v = Some(adrp_page(u, t) as u64);
        } else if let Some((_, n, imm)) = add_imm(u) {
            v = val.get(&n).map(|b| b + imm);
        } else if u & 0xFFE0_FFE0 == 0xAA00_03E0 {
            v = val.get(&((u >> 16) & 31)).copied();
        } else if u & 0x0A00_0000 == 0x0800_0000 {
            if (u >> 22) & 1 == 0 {
                t += 4;
                continue;
            }
            if u & 0x3A00_0000 == 0x2800_0000 {
                val.remove(&((u >> 10) & 31));
            }
        } else if u & 0x1C00_0000 == 0x1400_0000 {
            t += 4;
            continue;
        }
        if d == 1 {
            name = t == j;
        }
        match v {
            Some(v) => {
                val.insert(d, v);
            }
            None => {
                val.remove(&d);
            }
        }
        t += 4;
    }
    None
}

/// The add xD, xN, #lo (after an adrp xN of the string's page within 20 bytes) that make `s`'s address.
fn string_refs(m: &Main, s: usize) -> Vec<usize> {
    let lo = (s & 0xFFF) as u32;
    let mut out = vec![];
    let mut j = 0;
    while j + 4 <= m.text_end {
        let w = m.w(j);
        if w & 0xFFC0_0000 == 0x9100_0000 && (w >> 10) & 0xFFF == lo {
            let n = (w >> 5) & 31;
            if (j.saturating_sub(20)..j).step_by(4).any(|i| is_adrp(m.w(i)) && m.w(i) & 31 == n && adrp_page(m.w(i), i) == (s & !0xFFF) as i64) {
                out.push(j);
            }
        }
        j += 4;
    }
    out
}

pub fn ue_cvar(m: &Main, name: &str) -> Result<Option<u64>, String> {
    let mut objs = BTreeSet::new();
    for z in [2usize, 4] {
        let mut pat: Vec<u8> = if z == 2 {
            name.encode_utf16().flat_map(|c| c.to_le_bytes()).collect()
        } else {
            name.chars().flat_map(|c| (c as u32).to_le_bytes()).collect()
        };
        pat.extend(std::iter::repeat_n(0, z));
        for s in find_bytes(m.img, &pat, m.ro_off, m.data_off) {
            if s % z != 0 {
                continue;
            }
            for j in string_refs(m, s) {
                if let Some(o) = registered_at(m, j) {
                    objs.insert(o);
                }
            }
        }
    }
    match objs.len() {
        0 => Ok(None),
        1 => Ok(objs.into_iter().next()),
        _ => Err(format!("{name}: registered at {} objects ({})", objs.len(), objs.iter().map(|o| format!("{o:#x}")).collect::<Vec<_>>().join(", "))),
    }
}

// ---------------- Unreal fixed frame rate

/// (GEngine's offset, FixedFrameRate's offset in UEngine, bUseFixedFrameRate as byte offset << 8 | bit)
pub fn ue_fixed(m: &Main) -> Result<Option<(u64, u32, u32)>, String> {
    let rd32 = |o: usize| -> u32 { word(m.img, o) };
    let names = find_bytes(m.img, b"FixedFrameRate\0", m.ro_off, m.data_off);
    let mut fields = BTreeMap::new();
    for (&o, &v) in &m.rel {
        let o = o as usize;
        if !names.contains(&(v as usize)) || o < m.data_off || o + 64 > m.img.len() {
            continue;
        }
        if m.img[o + 24] != 0x0A || rd32(o + 32) != 1 {
            continue;
        }
        let f = match rd32(o + 36) {
            0 => rd32(o + 56),
            f => f,
        };
        if f != 0 && f % 4 == 0 && f < 0x4000 {
            fields.insert(o, f);
        }
    }
    if fields.is_empty() {
        return Ok(None);
    }
    let offs: BTreeSet<u32> = fields.values().copied().collect();
    if offs.len() > 1 {
        return Err(format!("UEngine::FixedFrameRate: float properties of that name at {}", offs.iter().map(|f| format!("+{f:#x}")).collect::<Vec<_>>().join(", ")));
    }
    let f = *offs.iter().next().unwrap();
    // the flag: its reflection entry (kind Bool) holds the set-bit function
    let s = find_bytes(m.img, b"bUseFixedFrameRate\0", m.ro_off, m.data_off).first().copied();
    let mut flag = None;
    for (&o, &v) in &m.rel {
        let o = o as usize;
        if Some(v as usize) != s || o < m.data_off || m.img.get(o + 24) != Some(&0x0C) {
            continue;
        }
        let Some(fnc) = (o + 8..o + 0x50).step_by(8).find_map(|k| m.rel.get(&(k as u64)).copied().filter(|&t| t > 0 && (t as usize) < m.text_end)) else { continue };
        let fnc = fnc as usize;
        let (ld, orr, st, ret) = (m.w(fnc), m.w(fnc + 4), m.w(fnc + 8), m.w(fnc + 12));
        if ld & 0xFFC0_03E0 != 0x3940_0000 || st != ld & !0x0040_0000 || ret != RET {
            continue;
        }
        let a = ld & 31;
        if orr & 0xFFC0_03FF != 0x3200_0000 | a << 5 | a || (orr >> 10) & 0x3F != 0 {
            continue;
        }
        let mask = 1u32 << ((32 - ((orr >> 16) & 0x3F)) % 32);
        if mask < 0x100 {
            flag = Some(((ld >> 10) & 0xFFF, mask));
            break;
        }
    }
    let Some(fl) = flag else { return Err(format!("UEngine::FixedFrameRate at +{f:#x}: no set-bit function of bUseFixedFrameRate of the known form")) };
    // GEngine: where the engine made under the "GameEngine" key is stored, loaded back and called
    let key: Vec<u8> = "GameEngine".encode_utf16().flat_map(|c| c.to_le_bytes()).chain([0, 0]).collect();
    let tw = |u: u32| -> Option<u32> {
        // fps_target.py's writes
        if u & 0x0A00_0000 == 0x0800_0000 {
            if (u >> 22) & 1 == 0 || (u >> 26) & 1 != 0 {
                return None;
            }
            return Some(u & 31);
        }
        if u & 0x1C00_0000 == 0x1400_0000 {
            return None;
        }
        if u & 0x1F00_0000 == 0x1E00_0000 && (u >> 29) & 1 == 0 && !matches!(u & 0xFFFF_FC00, 0x1E26_0000 | 0x9E66_0000) {
            return None;
        }
        Some(u & 31)
    };
    fn value(m: &Main, tw: &dyn Fn(u32) -> Option<u32>, i: usize, r: u32, depth: u32) -> Option<i64> {
        if depth > 3 {
            return None;
        }
        let mut k = i as i64 - 4;
        while k > (i as i64 - 0x4000).max(0) {
            let u = m.w(k as usize);
            if is_adrp(u) && u & 31 == r {
                return Some(adrp_page(u, k as usize));
            }
            if tw(u) == Some(r) {
                let n = (u >> 5) & 31;
                if u & 0xFFC0_0000 == 0x9100_0000 {
                    return value(m, tw, k as usize, n, depth + 1).map(|b| b + ((u >> 10) & 0xFFF) as i64);
                }
                if u & 0xFFC0_0000 == 0xF940_0000 {
                    let b = value(m, tw, k as usize, n, depth + 1)?;
                    return m.rel.get(&((b + ((u >> 10) & 0xFFF) as i64 * 8) as u64)).map(|&v| v as i64);
                }
                if u & 0xFFE0_FFE0 == 0xAA00_03E0 {
                    return value(m, tw, k as usize, (u >> 16) & 31, depth + 1);
                }
                return None;
            }
            k -= 4;
        }
        None
    }
    let mut found: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
    for s in find_bytes(m.img, &key, m.ro_off, m.data_off).into_iter().filter(|s| s % 2 == 0) {
        for r in string_refs(m, s) {
            let mut k = r + 4;
            while k < m.text_end.min(r + 0x100) {
                let u = m.w(k);
                if u & 0xFFC0_0000 != 0xF900_0000 {
                    k += 4;
                    continue;
                }
                let (rr, lo) = ((u >> 5) & 31, ((u >> 10) & 0xFFF) * 8);
                let reload = 0xF940_0000 | (lo / 8) << 10 | rr << 5;
                let at = (k + 4..m.text_end.min(k + 32)).step_by(4).find(|&q| m.w(q) == reload);
                if let Some(at) = at {
                    if !(k + 4..at).step_by(4).any(|q| tw(m.w(q)) == Some(rr)) && (at + 4..m.text_end.min(at + 24)).step_by(4).any(|q| m.w(q) & 0xFFFF_FC1F == 0xD63F_0000) {
                        if let Some(b) = value(m, &tw, k, rr, 0) {
                            let g = (b + lo as i64) as u64;
                            if g >= m.data_off as u64 && g % 8 == 0 {
                                found.entry(g).or_default().push(k);
                            }
                        }
                        break;
                    }
                }
                k += 4;
            }
        }
    }
    match found.len() {
        0 => Ok(None),
        1 => Ok(Some((*found.keys().next().unwrap(), f, fl.0 << 8 | fl.1))),
        _ => Err(format!("UEngine::FixedFrameRate: the engine creation names {} globals", found.len())),
    }
}

// ---------------- Monolith

struct Sig(usize, &'static str);
struct Impl {
    name: &'static str,
    wait: Sig,
    duration: Sig,
    setter: Sig,
    margin_literal: Option<(usize, usize)>,
}
const REVIEWED: [Impl; 3] = [
    Impl {
        name: "Monolith X",
        wait: Sig(292, "117445a6c8a8990b03e8a0e8af7a1e52dbee46f8efac38e7f1fb9596981ef86e"),
        duration: Sig(152, "74a04be38c6f48c37b3ad9a1e3e341e35d0d19f0802f15a46d218517ebef526e"),
        setter: Sig(32, "5b3276a0309456b63d7a5ebf4477d5b4249b49b769e9033d540beb29823345ce"),
        margin_literal: None,
    },
    Impl {
        name: "Monolith 2",
        wait: Sig(448, "86b7a37806e9cc4aa4840fe5517c9ee620c5becea0839ab48bbe2335bc788536"),
        duration: Sig(96, "e4096168156c605e698a4d5cadbb39819fcf1b6bbbc3f1621dbc4f1c25e569aa"),
        setter: Sig(32, "87d85cb2e136e4dd79434e7199db92e401b96aacce8ed6cb5dfa5fb816962dac"),
        margin_literal: Some((0x70, 0x7c)),
    },
    Impl {
        name: "Monolith 3",
        wait: Sig(292, "12c6abb0e0a13b604256f1d30b2d32c3f1f54d064e2d26112cf249e363af4a5f"),
        duration: Sig(100, "e71a92e3001a7622d9e715d446c065457ebfd2c4b0463b1894a65274c323c2e8"),
        setter: Sig(32, "87d85cb2e136e4dd79434e7199db92e401b96aacce8ed6cb5dfa5fb816962dac"),
        margin_literal: None,
    },
];
const PERIOD: u32 = 0x3c88_8889; // 1/60
const MARGIN: u32 = 0x3f7f_3b64; // 0.997

fn ldr64(w: u32) -> Option<(u32, u32, u64)> {
    (w & 0xFFC0_0000 == 0xF940_0000).then(|| (w & 31, (w >> 5) & 31, ((w >> 10) & 0xFFF) as u64 * 8))
}

/// The setter's two stores: the interval (from w0) and, beside it, the previous one (read from the interval).
pub fn monolith_setter_fields(m: &Main, start: usize, end: usize) -> Result<(u64, u64), String> {
    #[derive(Clone, Copy, PartialEq, Debug)]
    enum V {
        Page(i64),
        Addr(u64),
        Read(u64),
        Input,
    }
    let mut values: HashMap<u32, V> = HashMap::new();
    let mut writes: Vec<(u64, Option<V>)> = vec![];
    let mut at = start;
    while at < end {
        let w = m.w(at);
        if is_adrp(w) {
            values.insert(w & 31, V::Page(adrp_page(w, at)));
        } else if let Some((d, n, off)) = ldr64(w) {
            let Some(V::Page(p)) = values.get(&n).copied() else { return Err("setter has no GOT address".into()) };
            let t = m.rel.get(&((p + off as i64) as u64)).ok_or("setter GOT slot has no relative relocation")?;
            values.insert(d, V::Addr(*t));
        } else if w & 0xFFC0_0000 == 0xB940_0000 {
            match values.get(&((w >> 5) & 31)) {
                Some(V::Addr(a)) if (w >> 10) & 0xFFF == 0 => {
                    let a = *a;
                    values.insert(w & 31, V::Read(a));
                }
                _ => return Err("setter does not read the interval directly".into()),
            }
        } else if w & 0xFFC0_0000 == 0xB900_0000 {
            let Some(V::Addr(a)) = values.get(&((w >> 5) & 31)).copied().filter(|_| (w >> 10) & 0xFFF == 0) else {
                return Err("setter does not write the interval directly".into());
            };
            let src = if w & 31 == 0 { Some(V::Input) } else { values.get(&(w & 31)).copied() };
            writes.push((a, src));
        } else if !(w == RET && at == end - 4) {
            return Err("setter contains an unreviewed operation".into());
        }
        at += 4;
    }
    if writes.len() != 2 || writes[0].1 != Some(V::Input) || writes[1].1 != Some(V::Read(writes[0].0)) {
        return Err("setter does not save the old interval".into());
    }
    let (cur, prev) = (writes[0].0, writes[1].0);
    if prev != cur + 4 {
        return Err("previous interval is not beside the current one".into());
    }
    Ok((cur, prev))
}

fn got_refs(m: &Main, start: usize, end: usize) -> BTreeSet<u64> {
    let mut pages: HashMap<u32, i64> = HashMap::new();
    let mut refs = BTreeSet::new();
    let mut a = start;
    while a < end {
        let w = m.w(a);
        if let Some((_, n, off)) = ldr64(w) {
            if let Some(p) = pages.get(&n) {
                if let Some(&t) = m.rel.get(&((p + off as i64) as u64)) {
                    refs.insert(t);
                }
            }
        }
        for r in 0..31 {
            if writes_gpr(w, r) {
                pages.remove(&r);
            }
        }
        if is_adrp(w) {
            pages.insert(w & 31, adrp_page(w, a));
        }
        if w & 0xFC00_0000 == 0x9400_0000 || w & 0xFFFF_FC1F == 0xD63F_0000 {
            for r in 0..19 {
                pages.remove(&r);
            }
        } else if w & 0xFC00_0000 == 0x1400_0000 || w & 0xFFFF_FC1F == 0xD61F_0000 || w == RET {
            pages.clear();
        }
        a += 4;
    }
    refs
}

fn literal(m: &Main, page_at: usize, load_at: usize) -> Option<usize> {
    let p = m.w(page_at);
    let w = m.w(load_at);
    (is_adrp(p) && w & 0xFFC0_0000 == 0xBD40_0000 && (w >> 5) & 31 == p & 31).then(|| (adrp_page(p, page_at) + ((w >> 10) & 0xFFF) as i64 * 4) as usize)
}

/// The context method returns exactly 1/60 (a movz / movk / fmov, or a literal load).
pub fn monolith_period(m: &Main, start: usize, size: usize) -> bool {
    if size == 16 {
        let (a, b, c, d) = (m.w(start), m.w(start + 4), m.w(start + 8), m.w(start + 12));
        if a & 0xFFE0_0000 == 0x5280_0000 && b & 0xFFE0_0000 == 0x72A0_0000 && a & 31 == b & 31 && c == 0x1E27_0000 | (a & 31) << 5 && d == RET {
            return ((a >> 5) & 0xFFFF) | ((b >> 5) & 0xFFFF) << 16 == PERIOD;
        }
    }
    if size == 12 && m.w(start + 4) & 31 == 0 && m.w(start + 8) == RET {
        if let Some(l) = literal(m, start, start + 4) {
            return m.ro_off <= l && l + 4 <= m.data_off && word(m.img, l) == PERIOD;
        }
    }
    false
}

/// Monolith's frame interval (an int32 of vsyncs, 0 = 2): its offset in main.
pub fn monolith(m: &Main) -> Result<Option<u64>, String> {
    let Ok(mut img) = Image::new(m.img, m.text_end) else { return Ok(None) };
    let functions = img.functions.clone();
    let mut matches = |s: &Sig| -> Vec<usize> {
        functions.iter().filter(|(a, b)| *b - *a == s.0).filter(|(a, _)| img.fingerprint(**a).is_ok_and(|f| f == s.1)).map(|(a, _)| *a).collect()
    };
    let (mut found, mut failures) = (vec![], vec![]);
    for im in &REVIEWED {
        let waits = matches(&im.wait);
        if waits.is_empty() {
            continue;
        }
        let r: Result<u64, String> = (|| {
            if waits.len() != 1 {
                return Err(format!("{} matching waits", waits.len()));
            }
            let (dur, set) = (matches(&im.duration), matches(&im.setter));
            if dur.len() != 1 || set.len() != 1 {
                return Err(format!("{} duration getters and {} setters", dur.len(), set.len()));
            }
            let (wait, dur, set) = (waits[0], dur[0], set[0]);
            let (cur, prev) = monolith_setter_fields(m, set, functions[&set])?;
            if cur & 3 != 0 || (cur as usize) < m.data_off || cur as usize + 8 > m.img.len() {
                return Err("interval pair is outside writable module data".into());
            }
            if [cur, prev].iter().any(|&o| !(-1..=4).contains(&(word(m.img, o as usize) as i32))) {
                return Err("interval pair has unexpected initial values".into());
            }
            let shared: BTreeSet<u64> = got_refs(m, wait, functions[&wait]).intersection(&got_refs(m, dur, functions[&dur])).copied().collect();
            if !shared.contains(&cur) || shared.len() != 2 {
                return Err("wait and duration getter do not share the interval and context".into());
            }
            let ctx = *shared.iter().find(|&&s| s != cur).unwrap();
            if ctx & 7 != 0 || (ctx as usize) < m.data_off || ctx as usize + 8 > m.img.len() {
                return Err("context pointer is outside writable module data".into());
            }
            let has_table = m.rel.iter().any(|(&slot, &t)| {
                t == wait as u64 && slot as usize >= m.ro_off + 24 && m.rel.get(&(slot - 24)).is_some_and(|&p| functions.get(&(p as usize)).is_some_and(|&e| monolith_period(m, p as usize, e - p as usize)))
            });
            if !has_table {
                return Err("wait has no context vtable with a 1/60 period".into());
            }
            if let Some((pa, la)) = im.margin_literal {
                match literal(m, wait + pa, wait + la) {
                    Some(at) if m.ro_off <= at && at + 4 <= m.data_off && word(m.img, at) == MARGIN => {}
                    _ => return Err("wait margin literal is not 0.997".into()),
                }
            }
            Ok(cur)
        })();
        match r {
            Ok(c) => found.push(c),
            Err(e) => failures.push(format!("{}: {e}", im.name)),
        }
    }
    if !failures.is_empty() {
        return Err(format!("Monolith frame interval: {}", failures.join("; ")));
    }
    match found.len() {
        0 => Ok(None),
        1 => Ok(Some(found[0])),
        n => Err(format!("Monolith frame interval: {n} complete implementations qualify")),
    }
}

// ---------------- what nxrs-build writes

/// Every finder on main: the lines of `fps_lock.txt` (`<kind> <hex>...`) and what was said.
pub fn find_all(nso: &translator::loader::nso::Nso) -> (Vec<String>, Vec<String>) {
    let mut img = nso.image.clone();
    img.resize(nso.image.len() + nso.bss_size as usize, 0);
    let mut m = Main::of(nso);
    m.img = &img;
    let (mut lines, mut said) = (vec![], vec![]);
    let (u, why) = unity(&m);
    if why != "no Unity internal-call table" {
        said.push(why);
    }
    if let Some(t) = u.target {
        lines.push(format!("unity_target {t:#x}"));
    }
    if let Some((q, i, a, s, v)) = u.qs {
        lines.push(format!("unity_vsync {q:#x} {i:#x} {a:#x} {s:#x} {v:#x}"));
    }
    for n in UE_NAMES {
        match ue_cvar(&m, n) {
            Ok(Some(o)) => {
                lines.push(format!("ue_cvar {n} {o:#x}"));
                said.push(format!("Unreal {n} at {o:#x}"));
            }
            Ok(None) => {}
            Err(e) => said.push(format!("[RED] {e}")),
        }
    }
    match ue_fixed(&m) {
        Ok(Some((g, f, fl))) => {
            lines.push(format!("ue_fixed {g:#x} {f:#x} {fl:#x}"));
            said.push(format!("Unreal FixedFrameRate at [{g:#x}]+{f:#x}, flag {fl:#x}"));
        }
        Ok(None) => {}
        Err(e) => said.push(format!("[RED] {e}")),
    }
    match monolith(&m) {
        Ok(Some(o)) => {
            lines.push(format!("monolith {o:#x}"));
            said.push(format!("Monolith frame interval at {o:#x}"));
        }
        Ok(None) => {}
        Err(e) => said.push(format!("[RED] {e}")),
    }
    (lines, said)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn main_of(img: &[u8], text_end: usize, ro_off: usize, data_off: usize, rel: &[(u64, u64)]) -> Main<'static> {
        let img: &'static [u8] = Box::leak(img.to_vec().into_boxed_slice());
        Main { img, text_end, ro_off, data_off, rel: rel.iter().copied().collect() }
    }
    fn put(img: &mut Vec<u8>, at: usize, words: &[u32]) {
        for (i, w) in words.iter().enumerate() {
            img[at + 4 * i..at + 4 * i + 4].copy_from_slice(&w.to_le_bytes());
        }
    }
    fn adrp(pc: usize, target: usize, rd: u32) -> u32 {
        let imm = ((target >> 12) as i64 - (pc >> 12) as i64) as u32 & 0x1F_FFFF;
        0x9000_0000 | (imm & 3) << 29 | (imm >> 2) << 5 | rd
    }

    /// Anime Girls: Sun of a Beach (Unity 2021.3.28f1, the owner's build) when it is here: the finder reads its main.
    #[test]
    fn unity_on_anime_girls() {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../build/games/agsob/agsob_data/exefs/main");
        let Ok(raw) = std::fs::read(&p) else {
            eprintln!("no Anime Girls build here: skipped");
            return;
        };
        let nso = translator::loader::nso::Nso::parse(&raw).unwrap();
        let m = Main::of(&nso);
        let (u, why) = unity(&m);
        eprintln!("{why}");
        // Unity 2021.3.28f1: IL2CPP resolves the call by name at run time (main+0x1827400: adrp x0 / add x0, the
        // name / bl the resolver); the engine registers it in neither reviewed form, so nothing is written there
        assert_eq!((u, why.as_str()), (Unity::default(), "no Unity internal-call table"));
    }

    /// Unity's table: names then functions (relative relocations); the setter stores w0 at a global, the getter
    /// walks QualitySettings' levels.
    #[test]
    fn unity_by_its_internal_calls() {
        let (ro, data) = (0x4000usize, 0x8000usize);
        let mut img = vec![0u8; 0xA000];
        let names: [Vec<u8>; 3] = [b"UnityEngine.Foo::Bar\0".to_vec(), format!("{TFR}\0").into_bytes(), format!("{VSYNC}\0").into_bytes()];
        let mut at = ro + 1;
        let mut name_offs = vec![];
        img[ro] = 0;
        for n in &names {
            name_offs.push(at as u64);
            img[at..at + n.len()].copy_from_slice(n);
            at += n.len();
        }
        // the setter at 0x100: adrp x8, data page; str w0, [x8, #0x20]; ret
        put(&mut img, 0x100, &[adrp(0x100, data, 8), 0xB900_2100, RET]);
        // the getter at 0x200: adrp x8; ldr x8,[x8,#0x40] (QualitySettings); ldrsw x9,[x8,#0x64]; ldr x8,[x8,#0x20];
        // add x8, x8, x9, lsl #8; ldr w0, [x8, #0x88]; ret
        put(&mut img, 0x200, &[adrp(0x200, data, 8), 0xF940_2108, 0xB980_6509, 0xF940_1108, 0x8B09_2108, 0xB940_8900, RET]);
        let slots = data + 0x100;
        let mut rel: Vec<(u64, u64)> = name_offs.iter().enumerate().map(|(i, &o)| ((slots + 8 * i) as u64, o)).collect();
        rel.extend([(slots as u64 + 24, 0x80), (slots as u64 + 32, 0x100), (slots as u64 + 40, 0x200)]);
        let m = main_of(&img, 0x1000, ro, data, &rel);
        let (u, why) = unity(&m);
        assert_eq!(u, Unity { target: Some(data as u64 + 0x20), qs: Some((data as u64 + 0x40, 0x64, 0x20, 0x100, 0x88)) }, "{why}");
    }

    /// An Unreal console variable registered through IConsoleManager: name in x1, blr, the object's address, the
    /// value pointer stored at +0x10.
    #[test]
    fn ue_cvar_by_its_registration() {
        let (ro, data) = (0x4000usize, 0x8000usize);
        let mut img = vec![0u8; 0xA000];
        let name: Vec<u8> = "t.MaxFPS".encode_utf16().flat_map(|c| c.to_le_bytes()).chain([0, 0]).collect();
        img[ro + 0x10..ro + 0x10 + name.len()].copy_from_slice(&name);
        let obj = data + 0x300;
        // adrp x1, name page; add x1, x1, #0x10; blr x8; adrp x9, obj page; add x9, x9, #0x300; str x0, [x9, #0x10]; ret
        put(&mut img, 0x100, &[adrp(0x100, ro, 1), 0x9100_4021, 0xD63F_0100, adrp(0x10C, data, 9), 0x910C_0129, 0xF900_0920, RET]);
        let m = main_of(&img, 0x1000, ro, data, &[]);
        assert_eq!(ue_cvar(&m, "t.MaxFPS"), Ok(Some(obj as u64)));
        assert_eq!(ue_cvar(&m, "rhi.SyncInterval"), Ok(None));
    }

    /// UEngine::FixedFrameRate: the float property's reflection entry, the flag's set-bit function, GEngine where the
    /// engine made under "GameEngine" is stored, reloaded and called.
    #[test]
    fn ue_fixed_by_reflection_and_engine_creation() {
        let (ro, data) = (0x4000usize, 0x8000usize);
        let mut img = vec![0u8; 0xA000];
        img[ro + 0x10..ro + 0x10 + 15].copy_from_slice(b"FixedFrameRate\0");
        img[ro + 0x40..ro + 0x40 + 19].copy_from_slice(b"bUseFixedFrameRate\0");
        let key: Vec<u8> = "GameEngine".encode_utf16().flat_map(|c| c.to_le_bytes()).chain([0, 0]).collect();
        img[ro + 0x80..ro + 0x80 + key.len()].copy_from_slice(&key);
        // the float property entry at data+0x100: kind 0x0A at +24, array size 1 at +32, offset 0x7a8 at +36
        img[data + 0x100 + 24] = 0x0A;
        put(&mut img, data + 0x100 + 32, &[1, 0x7a8]);
        // the bool entry at data+0x200: kind 0x0C at +24, its set-bit function at +8
        img[data + 0x200 + 24] = 0x0C;
        // set-bit at 0x300: ldrb w8,[x0,#0x7a4]; orr w8,w8,#0x40; strb w8,[x0,#0x7a4]; ret
        put(&mut img, 0x300, &[0x395E_9008, 0x321A_0108, 0x391E_9008, RET]);
        // creation at 0x400: adrp x1; add x1,x1,#0x80 (key); bl; adrp x20, data; str x0,[x20,#0x880]; ldr x0,[x20,#0x880]; blr x8
        put(&mut img, 0x400, &[adrp(0x400, ro, 1), 0x9102_0021, 0x9400_0000, adrp(0x40C, data, 20), 0xF904_4280, 0xF944_4280, 0xD63F_0100]);
        let rel = [((data + 0x100) as u64, (ro + 0x10) as u64), ((data + 0x200) as u64, (ro + 0x40) as u64), ((data + 0x208) as u64, 0x300)];
        let m = main_of(&img, 0x1000, ro, data, &rel);
        assert_eq!(ue_fixed(&m), Ok(Some(((data + 0x880) as u64, 0x7a8, 0x7a4 << 8 | 0x40))));
    }

    /// Monolith's setter (the interval from w0, the previous one beside it) and its 1/60 period method; a main with
    /// no reviewed functions has no interval.
    #[test]
    fn monolith_parts() {
        let (ro, data) = (0x4000usize, 0x8000usize);
        let mut img = vec![0u8; 0xA000];
        // setter at 0x100: adrp x8; ldr x8,[x8,#0x10] (GOT -> data+0x500); ldr w9,[x8]; str w0,[x8]; adrp x10;
        // ldr x10,[x10,#0x18] (GOT -> data+0x504); str w9,[x10]; ret
        put(&mut img, 0x100, &[adrp(0x100, data, 8), 0xF940_0908, 0xB940_0109, 0xB900_0100, adrp(0x110, data, 10), 0xF940_0D4A, 0xB900_0149, RET]);
        let rel = [((data + 0x10) as u64, (data + 0x500) as u64), ((data + 0x18) as u64, (data + 0x504) as u64)];
        let m = main_of(&img, 0x1000, ro, data, &rel);
        assert_eq!(monolith_setter_fields(&m, 0x100, 0x120), Ok(((data + 0x500) as u64, (data + 0x504) as u64)));
        // period at 0x200: movz w8,#0x8889; movk w8,#0x3c88,lsl 16; fmov s0,w8; ret
        put(&mut img, 0x200, &[0x5291_1128, 0x72A7_9108, 0x1E27_0100, RET]);
        let m = main_of(&img, 0x1000, ro, data, &rel);
        assert!(monolith_period(&m, 0x200, 16));
        assert_eq!(monolith(&m), Ok(None));
    }
}
