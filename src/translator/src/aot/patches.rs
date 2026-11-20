//! Code patch table: guest instructions replaced in the lifted C, switchable at run time, plus spin-wait back
//! edges and spans skipped whole.
//!
//! File format (`#` starts a comment):
//!   set  <name> <env> <default>
//!   site <set> <module> <offset> <original> <replacement> [...]   (`@page:x<n>` = adrp x<n>, <patch page>)
//!   span <set> <module> <offset> <word> [...]
//!   spin <module> <head> <back-edge> [...]
//!   area <set> <module> <word> [...]                 (a stub's code, appended to the module's patch area)
//!   areloc <module> <index> <op> <target> <addend>   (word <index> of that area: op adrp / b / bl to
//!                                                     target state / area / module, plus addend)
//! A site's replacement word may be `@<op>:<target>:<addend>[:<word>]` (relocated like an area word, e.g.
//! `@b:area:0x96c`; an adrp's register comes from the word).
//! A module with an area gets a code module of its own, `<module>_stubs`, after the last module: the frame-step
//! state page (zeros, the host writes it) and then the area's code (frame-step and stub patches).
//! The player's mods arrive here as sets and sites too (src/cli `mods.rs`: one set per mod).

use crate::loader::{ModLayout, Module};
use std::collections::BTreeMap;
use std::path::Path;

pub const PAGE: u64 = 0x1000;

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct PatchError(pub String);

fn perr<T>(s: String) -> Result<T, PatchError> {
    Err(PatchError(s))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Repl {
    Word(u32),
    /// `@page:x<n>`
    Page(u32),
    /// `@<op>:<target>:<addend>`: the word relocated (its other bits kept)
    Reloc(u32, Reloc),
}

/// Where a relocated word points: the state page, the module's patch area, or the module itself, plus addend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reloc {
    pub op: RelocOp,
    pub target: Target,
    pub addend: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelocOp {
    Adrp,
    B,
    Bl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    State,
    Area,
    Module,
}

impl Reloc {
    pub fn parse(op: &str, target: &str, addend: &str) -> Option<Reloc> {
        let op = match op {
            "adrp" => RelocOp::Adrp,
            "b" => RelocOp::B,
            "bl" => RelocOp::Bl,
            _ => return None,
        };
        let target = match target {
            "state" => Target::State,
            "area" => Target::Area,
            "module" => Target::Module,
            _ => return None,
        };
        let neg = addend.starts_with('-');
        let v = hex(addend.trim_start_matches('-')).ok()? as i64;
        Some(Reloc { op, target, addend: if neg { -v } else { v } })
    }

    /// `w` (the recorded word) at `pc` pointing at the target.
    pub fn apply(&self, w: u32, pc: u64, state: u64, area: u64, module: u64) -> Result<u32, PatchError> {
        let to = match self.target {
            Target::State => state,
            Target::Area => area,
            Target::Module => module,
        }
        .wrapping_add(self.addend as u64);
        match self.op {
            RelocOp::Adrp => adrp(pc, to, w & 0x1F),
            RelocOp::B | RelocOp::Bl => {
                let d = to.wrapping_sub(pc) as i64;
                if d % 4 != 0 || !(-(1 << 27)..(1 << 27)).contains(&d) {
                    return perr(format!("{pc:#x}: a branch to {to:#x} out of reach"));
                }
                let op = if self.op == RelocOp::Bl { 0x9400_0000 } else { 0x1400_0000 };
                Ok(op | ((d >> 2) as u32 & 0x03FF_FFFF))
            }
        }
    }
}

/// A module's patch area: its set, its words, their relocations ({word index: reloc}).
#[derive(Debug, Clone, Default)]
pub struct Area {
    pub set: usize,
    pub words: Vec<u32>,
    pub relocs: BTreeMap<usize, Reloc>,
}

#[derive(Debug, Clone)]
pub struct Site {
    pub set: usize,
    pub repl: Vec<Repl>,
    pub orig: u32,
}

#[derive(Debug, Clone, Default)]
pub struct Table {
    pub sets: Vec<(String, String, i64)>,
    pub sites: BTreeMap<String, BTreeMap<u64, Site>>,
    pub page: u64,
    pub spins: BTreeMap<String, BTreeMap<u64, u64>>,
    pub spans: BTreeMap<String, BTreeMap<u64, (usize, Vec<u32>)>>,
    pub areas: BTreeMap<String, Area>,
    /// The frame-step state page (the first page of the first stubs module); 0 without areas.
    pub state: u64,
    /// {module: (its stubs module's base, the address of its code)}
    pub area_at: BTreeMap<String, (u64, u64)>,
}

pub fn adrp(pc: u64, target: u64, rd: u32) -> Result<u32, PatchError> {
    let imm = (target >> 12) as i64 - (pc >> 12) as i64;
    if !(-(1 << 20)..(1 << 20)).contains(&imm) {
        return perr(format!("{pc:#x}: patch page {target:#x} out of ADRP range"));
    }
    let imm = (imm & 0x1FFFFF) as u32;
    Ok(0x9000_0000 | (imm & 3) << 29 | (imm >> 2) << 5 | rd)
}

fn hex(s: &str) -> Result<u64, std::num::ParseIntError> {
    u64::from_str_radix(s.trim_start_matches("0x"), 16)
}

impl Table {
    /// The table with no sets: what gen.py writes when the patch file is empty.
    pub fn empty(mods: &[ModLayout]) -> Result<Table, PatchError> {
        Self::parse("", "<none>", mods)
    }

    pub fn load(path: &Path, mods: &[ModLayout]) -> Result<Table, PatchError> {
        let text = std::fs::read_to_string(path).map_err(|e| PatchError(format!("{}: {e}", path.display())))?;
        Self::parse(&text, &path.display().to_string(), mods)
    }

    pub fn parse(text: &str, path: &str, mods: &[ModLayout]) -> Result<Table, PatchError> {
        let mut t = Table::default();
        let mut index: BTreeMap<String, usize> = BTreeMap::new();
        let bases: BTreeMap<&str, (u64, u64)> = mods.iter().map(|m| (m.name.as_str(), (m.base, m.size))).collect();
        for (no, raw) in text.lines().enumerate() {
            let f: Vec<&str> = raw.split('#').next().unwrap().split_whitespace().collect();
            if f.is_empty() {
                continue;
            }
            let wh = format!("{path}:{}", no + 1);
            let bad = |_| PatchError(format!("{wh}: cannot parse {raw:?}"));
            match (f[0], f.len()) {
                ("set", 4) => {
                    if index.contains_key(f[1]) {
                        return perr(format!("{wh}: set {} defined twice", f[1]));
                    }
                    index.insert(f[1].into(), t.sets.len());
                    t.sets.push((f[1].into(), f[2].into(), f[3].parse().map_err(|_| PatchError(wh.clone()))?));
                }
                ("site", n) if n >= 6 => {
                    let Some(&k) = index.get(f[1]) else { return perr(format!("{wh}: unknown set {}", f[1])) };
                    if !bases.contains_key(f[2]) {
                        return perr(format!("{wh}: module {} not in the layout", f[2]));
                    }
                    let (off, orig) = (hex(f[3]).map_err(bad)?, hex(f[4]).map_err(bad)? as u32);
                    let mut repl = vec![];
                    for wd in &f[5..] {
                        repl.push(match wd.strip_prefix("@page:x") {
                            Some(r) => Repl::Page(r.parse().map_err(|_| PatchError(wh.clone()))?),
                            None if wd.starts_with('@') => {
                                let p: Vec<&str> = wd[1..].split(':').collect();
                                // @op:target:addend[:recorded word] (an adrp keeps the word's register)
                                let r = (p.len() == 3 || p.len() == 4)
                                    .then(|| Reloc::parse(p[0], p[1], p[2]))
                                    .flatten()
                                    .ok_or_else(|| PatchError(format!("{wh}: bad relocation {wd}")))?;
                                let w = match p.get(3) {
                                    Some(x) => hex(x).map_err(bad)? as u32,
                                    None => 0,
                                };
                                Repl::Reloc(w, r)
                            }
                            None => Repl::Word(hex(wd).map_err(bad)? as u32),
                        });
                    }
                    if t.sites.get(f[2]).is_some_and(|m| m.contains_key(&off)) {
                        return perr(format!("{wh}: {}+{off:#x} patched twice", f[2]));
                    }
                    for (o, (_, ws)) in t.spans.get(f[2]).into_iter().flatten() {
                        if *o <= off && off < o + 4 * ws.len() as u64 {
                            return perr(format!("{wh}: {}+{off:#x} is both a site and in a span", f[2]));
                        }
                    }
                    t.sites.entry(f[2].into()).or_default().insert(off, Site { set: k, repl, orig });
                }
                ("span", n) if n >= 6 => {
                    let Some(&k) = index.get(f[1]) else { return perr(format!("{wh}: unknown set {}", f[1])) };
                    if !bases.contains_key(f[2]) {
                        return perr(format!("{wh}: module {} not in the layout", f[2]));
                    }
                    let off = hex(f[3]).map_err(bad)?;
                    let words = f[4..].iter().map(|x| hex(x).map(|v| v as u32)).collect::<Result<Vec<_>, _>>().map_err(bad)?;
                    let m = t.spans.entry(f[2].into()).or_default();
                    if m.contains_key(&off) {
                        return perr(format!("{wh}: {}+{off:#x} spanned twice", f[2]));
                    }
                    for (o, (_, ws)) in m.iter() {
                        if off < o + 4 * ws.len() as u64 && *o < off + 4 * words.len() as u64 {
                            return perr(format!("{wh}: {}+{off:#x} overlaps the span at {o:#x}", f[2]));
                        }
                    }
                    for o in t.sites.get(f[2]).into_iter().flat_map(|m| m.keys()) {
                        if off <= *o && *o < off + 4 * words.len() as u64 {
                            return perr(format!("{wh}: {}+{o:#x} is both a site and in a span", f[2]));
                        }
                    }
                    m.insert(off, (k, words));
                }
                ("area", n) if n >= 4 => {
                    let Some(&k) = index.get(f[1]) else { return perr(format!("{wh}: unknown set {}", f[1])) };
                    if !bases.contains_key(f[2]) {
                        return perr(format!("{wh}: module {} not in the layout", f[2]));
                    }
                    let a = t.areas.entry(f[2].into()).or_insert_with(|| Area { set: k, ..Default::default() });
                    for x in &f[3..] {
                        a.words.push(hex(x).map_err(bad)? as u32);
                    }
                }
                ("areloc", 6) => {
                    let Some(a) = t.areas.get_mut(f[1]) else { return perr(format!("{wh}: no area of {} before it", f[1])) };
                    let i: usize = f[2].parse().map_err(|_| PatchError(wh.clone()))?;
                    let r = Reloc::parse(f[3], f[4], f[5]).ok_or_else(|| PatchError(format!("{wh}: bad relocation")))?;
                    if i >= a.words.len() {
                        return perr(format!("{wh}: word {i} past the area's {} words", a.words.len()));
                    }
                    a.relocs.insert(i, r);
                }
                ("spin", n) if n >= 4 => {
                    if !bases.contains_key(f[1]) {
                        return perr(format!("{wh}: module {} not in the layout", f[1]));
                    }
                    let head = hex(f[2]).map_err(bad)?;
                    let m = t.spins.entry(f[1].into()).or_default();
                    for b in &f[3..] {
                        let b = hex(b).map_err(bad)?;
                        if m.insert(b, head).is_some() {
                            return perr(format!("{wh}: {}+{b:#x} is a back-edge twice", f[1]));
                        }
                    }
                }
                _ => return perr(format!("{wh}: cannot parse {raw:?}")),
            }
        }
        let Some(&(base, size)) = bases.get("main") else { return perr("no main module in the layout".into()) };
        let page = (base + size + PAGE - 1) & !(PAGE - 1);
        let nxt = mods.iter().map(|m| m.base).filter(|&b| b > base).min();
        if let Some(nxt) = nxt {
            if page + PAGE > nxt {
                return perr(format!("no room for the patch page after main ({page:#x})"));
            }
        }
        t.page = page;
        // the stubs modules: after every module (the layout packs them, NROs included) and the patch page, each a
        // state page then its code, 2 MiB aligned as modules are
        let mut at = mods.iter().map(|m| m.base + m.size).max().unwrap_or(0).max(page + PAGE);
        for (name, a) in &t.areas {
            let base = (at + 0x1F_FFFF) & !0x1F_FFFF;
            if t.state == 0 {
                t.state = base;
            }
            t.area_at.insert(name.clone(), (base, base + PAGE));
            at = base + PAGE + 4 * a.words.len() as u64;
        }
        Ok(t)
    }

    /// The stubs module of `name`'s area: (its name, base, size, code address, the relocated words).
    pub fn stubs_module(&self, name: &str, module_base: u64) -> Result<Option<(String, u64, u64, u64, Vec<u32>)>, PatchError> {
        let (Some(a), Some(&(base, code))) = (self.areas.get(name), self.area_at.get(name)) else { return Ok(None) };
        let mut words = a.words.clone();
        for (&i, r) in &a.relocs {
            words[i] = r.apply(words[i], code + 4 * i as u64, self.state, code, module_base)?;
        }
        Ok(Some((format!("{name}_stubs"), base, PAGE + 4 * words.len() as u64, code, words)))
    }

    /// Addresses of `name` that its area's code or its sites branch to (B targets, the return points after B
    /// sites): function starts of the module, as the dispatcher enters only those.
    pub fn branch_ins(&self, name: &str, base: u64) -> Vec<u64> {
        let mut out = vec![];
        if let Some(a) = self.areas.get(name) {
            out.extend(
                a.relocs.values().filter(|r| r.op == RelocOp::B && r.target == Target::Module).map(|r| base.wrapping_add(r.addend as u64)),
            );
        }
        for (off, s) in self.sites.get(name).into_iter().flatten() {
            if matches!(s.repl.last(), Some(Repl::Reloc(_, r)) if r.op == RelocOp::B) {
                out.push(base + off + 4 * s.repl.len() as u64);
            }
        }
        out
    }

    /// {absolute back-edge: absolute head}.
    pub fn spins_for(&self, name: &str, base: u64) -> BTreeMap<u64, u64> {
        self.spins.get(name).into_iter().flatten().map(|(b, h)| (base + b, base + h)).collect()
    }

    /// {absolute span start: (set, absolute span end)}.
    pub fn spans_for(&self, name: &str, base: u64) -> BTreeMap<u64, (usize, u64)> {
        self.spans
            .get(name)
            .into_iter()
            .flatten()
            .map(|(o, (k, ws))| (base + o, (*k, base + o + 4 * ws.len() as u64)))
            .collect()
    }

    /// {absolute address: (set, [words])} with @page resolved.
    pub fn for_module(&self, name: &str, base: u64) -> Result<BTreeMap<u64, (usize, Vec<u32>)>, PatchError> {
        let mut out = BTreeMap::new();
        for (off, s) in self.sites.get(name).into_iter().flatten() {
            let a = base + off;
            let ws = s
                .repl
                .iter()
                .map(|r| match r {
                    Repl::Word(w) => Ok(*w),
                    Repl::Page(rd) => adrp(a, self.page, *rd),
                    Repl::Reloc(w, r) => r.apply(*w, a, self.state, self.area_at.get(name).map_or(0, |x| x.1), base),
                })
                .collect::<Result<Vec<_>, _>>()?;
            out.insert(a, (s.set, ws));
        }
        Ok(out)
    }

    /// Stops on a site or span whose original words differ from the module's, or a bad spin back-edge.
    pub fn check(&self, m: &Module, name: &str) -> Result<(), PatchError> {
        for (off, s) in self.sites.get(name).into_iter().flatten() {
            let a = m.base + off;
            if !m.in_text(a) {
                return perr(format!("{name}+{off:#x}: patch site outside .text"));
            }
            let have = m.word(a);
            if have != s.orig {
                return perr(format!("{name}+{off:#x}: module has {have:08x}, the patch expects {:08x}", s.orig));
            }
        }
        for (off, (_, words)) in self.spans.get(name).into_iter().flatten() {
            for (i, want) in words.iter().enumerate() {
                let a = m.base + off + 4 * i as u64;
                if !m.in_text(a) {
                    return perr(format!("{name}+{off:#x}: patch span outside .text"));
                }
                let have = m.word(a);
                if have != *want {
                    return perr(format!(
                        "{name}+{:#x}: module has {have:08x}, the span expects {want:08x}",
                        off + 4 * i as u64
                    ));
                }
            }
        }
        for (b, head) in self.spins.get(name).into_iter().flatten() {
            let a = m.base + b;
            if !m.in_text(a) || branch_target(m.word(a), a) != Some(m.base + head) {
                return perr(format!("{name}+{b:#x}: not a direct branch to the spin wait head {name}+{head:#x}"));
            }
        }
        Ok(())
    }

    /// patches.h for the runtime.
    pub fn header_text(&self, marker: &str) -> String {
        let sets: Vec<String> =
            self.sets.iter().enumerate().map(|(i, (n, e, d))| format!("X({i}, {n}, \"{e}\", {d})")).collect();
        [
            format!("// {marker}: code patch sets, generated by recomp/gen.py --patches. Do not edit or commit."),
            "#pragma once".into(),
            String::new(),
            format!("#define RT_PATCH_COUNT {}", self.sets.len()),
            format!("#define RT_PATCH_PAGE {:#x}ull", self.page),
            "// X(index, name, env switch, default)".into(),
            format!("#define RT_PATCH_SETS(X) {}", sets.join(" ")),
            String::new(),
            "// the enabled code mods' bytes (gen.py --image): X(module, offset, \"bytes\", count)".into(),
            "#define RT_IMAGE_PATCH_COUNT 0".into(),
            "#define RT_IMAGE_PATCHES(X) ".into(),
            String::new(),
        ]
        .join("\n")
    }
}

/// Target of the direct branch w at a (b, b.cond, cbz/cbnz, tbz/tbnz), or None.
pub fn branch_target(w: u32, a: u64) -> Option<u64> {
    let s = |v: u32, n: u32| -> i64 {
        let v = v as i64;
        if v & (1 << (n - 1)) != 0 { v - (1 << n) } else { v }
    };
    if w >> 26 == 0x05 {
        return Some(a.wrapping_add((s(w & 0x3FF_FFFF, 26) << 2) as u64));
    }
    if w & 0xFF00_0010 == 0x5400_0000 || w & 0x7E00_0000 == 0x3400_0000 {
        return Some(a.wrapping_add((s((w >> 5) & 0x7FFFF, 19) << 2) as u64));
    }
    if w & 0x7E00_0000 == 0x3600_0000 {
        return Some(a.wrapping_add((s((w >> 5) & 0x3FFF, 14) << 2) as u64));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mods() -> Vec<ModLayout> {
        vec![ModLayout { name: "main".into(), base: 0x70_0000_0000, size: 0x10_0000 }]
    }

    #[test]
    fn parse_table() {
        let t = Table::parse(
            "set zstd MKW_ZSTD 1\nsite zstd main 100 94000001 d503201f # nop\nspin main 200 210 220\n",
            "t",
            &mods(),
        )
        .unwrap();
        assert_eq!(t.sets.len(), 1);
        assert_eq!(t.page, 0x70_0010_0000);
        assert_eq!(t.spins_for("main", 0x70_0000_0000).len(), 2);
        assert!(t.header_text("M").contains("X(0, zstd, \"MKW_ZSTD\", 1)"));
    }

    /// A frame-step stub: its area after main's patch page, the area's words relocated, the site's branch into it.
    #[test]
    fn stub_area() {
        let t = Table::parse(
            "set fstep NXRS_PATCH_FSTEP 1\n\
             area fstep main 90ff8010 b9400211 17ffffff 14000000\n\
             areloc main 0 adrp state 0\nareloc main 3 b module 0x104\n\
             site fstep main 100 681242b9 @b:area:0x4\n",
            "t",
            &mods(),
        )
        .unwrap();
        let main = 0x70_0000_0000u64;
        let (name, base, size, code, words) = t.stubs_module("main", main).unwrap().unwrap();
        assert_eq!((name.as_str(), base, size, code), ("main_stubs", 0x70_0020_0000, PAGE + 16, 0x70_0020_1000));
        assert_eq!(t.state, base);
        assert_eq!(words[0], adrp(code, base, 16).unwrap(), "adrp x16, state page");
        assert_eq!((words[1], words[2]), (0xb9400211, 0x17ffffff), "unrelocated words stay");
        assert_eq!(branch_target(words[3], code + 12), Some(main + 0x104), "back into main");
        let site = t.for_module("main", main).unwrap()[&(main + 0x100)].1[0];
        assert_eq!(branch_target(site, main + 0x100), Some(code + 4), "the site branches to the area");
        assert_eq!(t.branch_ins("main", main), vec![main + 0x104, main + 0x104]);
        assert!(Table::parse("set s E 1\narea s main 0\nareloc main 1 b area 0\n", "t", &mods()).is_err(), "word past the area");
    }

    #[test]
    fn adrp_encoding() {
        // adrp x3, page+0x1000 from page
        assert_eq!(adrp(0x1000, 0x2000, 3).unwrap(), 0xb0000003);
        assert_eq!(branch_target(0x17ffffff, 0x100), Some(0xfc));
    }
}
