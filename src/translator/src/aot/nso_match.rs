//! Function matching for patches across game builds (nxnative `dm/tools/nso_match.py`, reimplemented): a patch
//! rule names the function it changes by its size and fingerprint, so it applies to a build nobody registered.
//!
//! Functions are the unwind table's (every FDE's pc begin and range); no boundary is guessed. A function's body
//! digest is the sha256 of its instruction words with what moves between builds cleared: the immediates of direct
//! branches that leave the function, of ADR / ADRP and literal loads that point outside it (ADRP always), and the
//! low 12 bits an ADRP's address gets from its ADD / LDR / STR within the next 8 instructions. Its fingerprint is
//! the sha256 of the JSON `[body digest, [[offset, callee], ...]]` for those outside branches, a callee being
//! `["function", size, body digest]`, `["import", name]` (a PLT entry) or `["svc", n]` (an SDK wrapper
//! `svc #n; ret`). A rule applies where exactly one function of its size has one of its fingerprints, and each
//! cut's original bytes are checked there.

use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchError(pub String);

impl std::fmt::Display for MatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn merr<T>(s: String) -> Result<T, MatchError> {
    Err(MatchError(s))
}

fn signed(n: i64, bits: u32) -> i64 {
    if n & (1 << (bits - 1)) != 0 {
        n - (1 << bits)
    } else {
        n
    }
}

/// (immediate mask, byte displacement) of a direct branch (B / BL, B.cond, CBZ / CBNZ, TBZ / TBNZ).
fn relative(w: u32) -> Option<(u32, i64)> {
    if w & 0x7c00_0000 == 0x1400_0000 {
        return Some((0x03ff_ffff, signed((w & 0x3ff_ffff) as i64, 26) * 4));
    }
    if w & 0xff00_0010 == 0x5400_0000 || w & 0x7e00_0000 == 0x3400_0000 {
        return Some((0x00ff_ffe0, signed(((w >> 5) & 0x7ffff) as i64, 19) * 4));
    }
    if w & 0x7e00_0000 == 0x3600_0000 {
        return Some((0x0007_ffe0, signed(((w >> 5) & 0x3fff) as i64, 14) * 4));
    }
    None
}

fn is_br_blr_ret(w: u32) -> bool {
    matches!(w & 0xffff_fc1f, 0xd61f_0000 | 0xd63f_0000 | 0xd65f_0000)
}

/// Whether instruction `w` writes general register `r` (x / w), as the disassembler's register access reports it.
pub fn writes_gpr(w: u32, r: u32) -> bool {
    let rd = w & 31;
    let rt2 = (w >> 10) & 31;
    let rn = (w >> 5) & 31;
    let op0 = (w >> 25) & 0xf;
    // loads and stores: x1x0
    if op0 & 0b0101 == 0b0100 {
        let v = (w >> 26) & 1 == 1;
        if (w >> 27) & 7 == 0b111 {
            let load = (w >> 22) & 3 != 0;
            // atomic memory operations (LDADD, SWP, ...): write Rt
            if (w >> 24) & 1 == 0 && (w >> 21) & 1 == 1 && (w >> 10) & 3 == 0 {
                return !v && rd == r;
            }
            // pre / post index: the base is written back
            let wb = (w >> 24) & 1 == 0 && (w >> 21) & 1 == 0 && matches!((w >> 10) & 3, 1 | 3);
            let prfm = (w >> 30) & 3 == 3 && (w >> 22) & 3 == 2 && !v;
            return (wb && rn == r) || (load && !v && !prfm && rd == r);
        }
        if (w >> 27) & 7 == 0b101 {
            let load = (w >> 22) & 1 == 1;
            let wb = matches!((w >> 23) & 3, 1 | 3);
            return (wb && rn == r) || (load && !v && (rd == r || rt2 == r));
        }
        if (w >> 27) & 7 == 0b011 && (w >> 24) & 1 == 0 {
            // literal load (not PRFM)
            return !v && (w >> 30) & 3 != 3 && rd == r;
        }
        if (w >> 24) & 0x3f == 0b001000 {
            // exclusives: loads write Rt (and Rt2), stores their status register Rs
            let load = (w >> 22) & 1 == 1;
            let rs = (w >> 16) & 31;
            return if load {
                rd == r || ((w >> 21) & 1 == 1 && rt2 == r)
            } else {
                (w >> 23) & 1 == 0 && rs == r
            };
        }
        return false;
    }
    // data processing, immediate
    if (w >> 26) & 7 == 0b100 {
        return rd == r;
    }
    // data processing, register
    if (w >> 25) & 7 == 0b101 {
        // conditional compare writes only the flags
        if (w >> 21) & 0xff == 0b1101_0010 {
            return false;
        }
        return rd == r;
    }
    // branches, exceptions, system
    if (w >> 26) & 7 == 0b101 {
        if w & 0xfc00_0000 == 0x9400_0000 || w & 0xffff_fc1f == 0xd63f_0000 {
            return r == 30; // BL / BLR write the link register
        }
        if w & 0xfff0_0000 == 0xd530_0000 {
            return rd == r; // MRS
        }
        return false;
    }
    // FP / SIMD to general registers
    if (w >> 24) & 0x5f == 0b0001_1110 && (w >> 21) & 1 == 1 && (w >> 10) & 0x3f == 0 {
        let opcode = (w >> 16) & 7;
        // FCVT*S / FCVT*U (0, 1), FCVTA* (4, 5), FMOV to a general register (6)
        return matches!(opcode, 0 | 1 | 4 | 5 | 6) && rd == r;
    }
    if w & 0xbfe0_8400 == 0x0e00_0400 && matches!((w >> 11) & 0xf, 0b0101 | 0b0111) {
        return rd == r; // SMOV / UMOV
    }
    false
}

/// Function ranges (module offsets) from the unwind table, sorted; empty for a module without one.
pub fn fde_ranges(img: &[u8]) -> Result<Vec<(usize, usize)>, MatchError> {
    let rd32 = |o: usize| {
        img.get(o..o + 4)
            .map(|s| u32::from_le_bytes(s.try_into().unwrap()))
            .ok_or(MatchError(format!("read past the image at {o:#x}")))
    };
    let m0 = rd32(4)? as usize;
    if img.get(m0..m0 + 4) != Some(b"MOD0") {
        return merr("no MOD0".into());
    }
    let ehs = (m0 as i64 + rd32(m0 + 16)? as i32 as i64) as usize;
    let ehe = (m0 as i64 + rd32(m0 + 20)? as i32 as i64) as usize;
    let read = |o: usize, enc: u8| -> Result<(i64, usize), MatchError> {
        let (fmt, app) = (enc & 0xf, enc & 0x70);
        let g = |n: usize| {
            img.get(o..o + n)
                .ok_or(MatchError(format!("pointer past the image at {o:#x}")))
        };
        let (mut v, sz) = match fmt {
            0x1 => {
                let (mut v, mut sh, mut p) = (0i64, 0, o);
                loop {
                    let b = *img.get(p).ok_or(MatchError("uleb past the image".into()))?;
                    v |= ((b & 0x7f) as i64) << sh;
                    sh += 7;
                    p += 1;
                    if b & 0x80 == 0 {
                        break;
                    }
                }
                (v, p - o)
            }
            0x3 => (u32::from_le_bytes(g(4)?.try_into().unwrap()) as i64, 4),
            0xb => (i32::from_le_bytes(g(4)?.try_into().unwrap()) as i64, 4),
            0x0 | 0x4 => (u64::from_le_bytes(g(8)?.try_into().unwrap()) as i64, 8),
            0xc => (i64::from_le_bytes(g(8)?.try_into().unwrap()), 8),
            _ => return merr(format!("pointer encoding {enc:#x} at {o:#x}")),
        };
        if app == 0x10 {
            v += o as i64;
        } else if app == 0x30 {
            v += ehs as i64;
        }
        Ok((v, sz))
    };
    if ehe < ehs + 12
        || img.get(ehs) != Some(&1)
        || matches!(img.get(ehs + 3), Some(0xff) | Some(0) | None)
    {
        return Ok(vec![]);
    }
    let (penc, cenc, tenc) = (img[ehs + 1], img[ehs + 2], img[ehs + 3]);
    let mut o = ehs + 4;
    o += read(o, penc)?.1;
    let (n, s) = read(o, cenc)?;
    o += s;
    let mut cies: HashMap<usize, u8> = HashMap::new();
    let mut out = vec![];
    for _ in 0..n.max(0) {
        let (_, s) = read(o, tenc)?;
        o += s;
        let (f, s) = read(o, tenc)?;
        o += s;
        let f = f as usize;
        let cptr = f + 4;
        let cie = cptr - rd32(cptr)? as usize;
        let enc = match cies.get(&cie) {
            Some(&e) => e,
            None => {
                let mut p = cie + 8;
                let ver = *img.get(p).ok_or(MatchError("CIE past the image".into()))?;
                p += 1;
                let end = img[p..]
                    .iter()
                    .position(|&b| b == 0)
                    .ok_or(MatchError("CIE augmentation".into()))?
                    + p;
                let aug = img[p..end].to_vec();
                p = end + 1;
                let uleb = |p: &mut usize| {
                    let mut v = 0u64;
                    let mut sh = 0;
                    loop {
                        let b = img[*p];
                        *p += 1;
                        v |= ((b & 0x7f) as u64) << sh;
                        sh += 7;
                        if b & 0x80 == 0 {
                            break v;
                        }
                    }
                };
                uleb(&mut p); // code alignment
                while img[p] & 0x80 != 0 {
                    p += 1; // data alignment (sleb)
                }
                p += 1;
                if ver == 1 {
                    p += 1;
                } else {
                    uleb(&mut p);
                }
                let mut enc = 0u8;
                if aug.first() == Some(&b'z') {
                    uleb(&mut p);
                    for &ch in &aug[1..] {
                        match ch {
                            b'R' => {
                                enc = img[p];
                                p += 1;
                            }
                            b'P' => {
                                let pe = img[p];
                                p += 1;
                                p += read(p, pe)?.1;
                            }
                            b'L' => p += 1,
                            _ => {}
                        }
                    }
                }
                cies.insert(cie, enc);
                enc
            }
        };
        let (st, s) = read(f + 8, enc)?;
        let (ln, _) = read(f + 8 + s, enc & 0xf)?;
        out.push((st as usize, (st + ln) as usize));
    }
    out.sort();
    Ok(out)
}

/// A module's image (offset 0 = the start of .text) prepared for matching.
pub struct Image<'a> {
    image: &'a [u8],
    text_end: usize,
    pub functions: BTreeMap<usize, usize>,
    bodies: HashMap<usize, (String, Vec<(usize, usize)>)>,
    prints: HashMap<usize, Option<String>>,
    imports: Option<HashMap<u64, String>>,
}

#[derive(Debug, Clone, PartialEq)]
enum Callee {
    Function(usize, String),
    Import(String),
    Svc(u32),
}

impl<'a> Image<'a> {
    pub fn new(image: &'a [u8], text_end: usize) -> Result<Image<'a>, MatchError> {
        let mut functions = BTreeMap::new();
        for (s, e) in fde_ranges(image)
            .map_err(|e| MatchError(format!("cannot read function boundaries: {e}")))?
        {
            if !(s < e && e <= text_end.min(image.len())) || s % 4 != 0 || e % 4 != 0 {
                continue;
            }
            if let Some(&old) = functions.get(&s) {
                if old != e {
                    return merr(format!("conflicting function boundaries at {s:#x}"));
                }
            }
            functions.insert(s, e);
        }
        Ok(Image {
            image,
            text_end,
            functions,
            bodies: HashMap::new(),
            prints: HashMap::new(),
            imports: None,
        })
    }

    fn word(&self, o: usize) -> u32 {
        u32::from_le_bytes(self.image[o..o + 4].try_into().unwrap())
    }

    /// (body digest, [(offset in the function, target)] of the branches that leave it).
    fn body(&mut self, start: usize) -> (String, Vec<(usize, usize)>) {
        if let Some(b) = self.bodies.get(&start) {
            return b.clone();
        }
        let end = self.functions[&start];
        let words: Vec<u32> = (start..end).step_by(4).map(|o| self.word(o)).collect();
        let mut norm = words.clone();
        let mut branches = vec![];
        for (i, &w) in words.iter().enumerate() {
            let pc = (start + 4 * i) as i64;
            let inside = |t: i64| (start as i64..end as i64).contains(&t);
            if let Some((mask, delta)) = relative(w) {
                let t = pc + delta;
                if !inside(t) {
                    norm[i] &= !mask;
                    branches.push((i * 4, t as usize));
                }
            } else if w & 0x1f00_0000 == 0x1000_0000 {
                let delta = signed((((w >> 29) & 3) | ((w >> 5) & 0x7ffff) << 2) as i64, 21);
                if w & 0x8000_0000 != 0 || !inside(pc + delta) {
                    norm[i] &= 0x9f00_001f;
                }
            } else if w & 0x3b00_0000 == 0x1800_0000 {
                let t = pc + signed(((w >> 5) & 0x7ffff) as i64, 19) * 4;
                if !inside(t) {
                    norm[i] &= 0xff00_001f;
                }
            }
            if w & 0x9f00_0000 != 0x9000_0000 {
                continue;
            }
            let rd = w & 31;
            for j in i + 1..(i + 9).min(words.len()) {
                let u = words[j];
                if (u & 0xffc0_0000 == 0x9100_0000 || u & 0x3b00_0000 == 0x3900_0000)
                    && (u >> 5) & 31 == rd
                {
                    norm[j] &= !0x3f_fc00;
                    break;
                }
                if relative(u).is_some() || is_br_blr_ret(u) || writes_gpr(u, rd) {
                    break;
                }
            }
        }
        let bytes: Vec<u8> = norm.iter().flat_map(|w| w.to_le_bytes()).collect();
        let digest = hex(&Sha256::digest(&bytes));
        self.bodies
            .insert(start, (digest.clone(), branches.clone()));
        (digest, branches)
    }

    /// GOT slot -> imported symbol from DT_JMPREL.
    fn import_slots(&mut self) -> &HashMap<u64, String> {
        if self.imports.is_none() {
            let img = self.image;
            let mut out = HashMap::new();
            let r = (|| -> Option<()> {
                let r32 = |o: usize| {
                    img.get(o..o + 4)
                        .map(|s| u32::from_le_bytes(s.try_into().unwrap()))
                };
                let r64 = |o: usize| {
                    img.get(o..o + 8)
                        .map(|s| u64::from_le_bytes(s.try_into().unwrap()))
                };
                let m = r32(4)? as usize;
                let dyn_ = (m as i64 + r32(m + 4)? as i32 as i64) as usize;
                let mut tags: HashMap<i64, u64> = HashMap::new();
                let mut pos = dyn_;
                while pos + 16 <= img.len() && pos < dyn_ + 4096 * 16 {
                    let tag = r64(pos)? as i64;
                    if tag == 0 {
                        break;
                    }
                    tags.insert(tag, r64(pos + 8)?);
                    pos += 16;
                }
                if tags.get(&20) != Some(&7) {
                    return Some(());
                }
                let (jmprel, sz, symtab, strtab) = (
                    *tags.get(&23)? as usize,
                    *tags.get(&2)? as usize,
                    *tags.get(&6)? as usize,
                    *tags.get(&5)? as usize,
                );
                let mut p = jmprel;
                while p < jmprel + sz {
                    let (slot, info) = (r64(p)?, r64(p + 8)?);
                    p += 24;
                    if info & 0xffff_ffff != 1026 {
                        continue;
                    }
                    let name = r32(symtab + (info >> 32) as usize * 24)? as usize + strtab;
                    let end = img[name..].iter().position(|&b| b == 0)? + name;
                    out.insert(slot, String::from_utf8_lossy(&img[name..end]).into_owned());
                }
                Some(())
            })();
            let _ = r;
            self.imports = Some(out);
        }
        self.imports.as_ref().unwrap()
    }

    fn plt_import(&mut self, target: usize) -> Option<String> {
        if target + 16 > self.text_end.min(self.image.len()) {
            return None;
        }
        let (a, b, c, d) = (
            self.word(target),
            self.word(target + 4),
            self.word(target + 8),
            self.word(target + 12),
        );
        if a & 0x9f00_001f == 0x9000_0010
            && b & 0xffc0_03ff == 0xf940_0211
            && c & 0xffc0_03ff == 0x9100_0210
            && d == 0xd61f_0220
        {
            let page = (target as i64 & !0xfff)
                + (signed((((a >> 29) & 3) | ((a >> 5) & 0x7ffff) << 2) as i64, 21) << 12);
            let lo = ((b >> 10) & 0xfff) as i64 * 8;
            if ((c >> 10) & 0xfff) as i64 == lo {
                return self.import_slots().get(&((page + lo) as u64)).cloned();
            }
        }
        None
    }

    fn callee(&mut self, target: usize) -> Result<Callee, MatchError> {
        if target + 16 <= self.text_end.min(self.image.len()) {
            if let Some(n) = self.plt_import(target) {
                return Ok(Callee::Import(n));
            }
            let (a, b) = (self.word(target), self.word(target + 4));
            if a & 0xffe0_001f == 0xd400_0001
                && b == 0xd65f_03c0
                && !self.functions.contains_key(&target)
            {
                return Ok(Callee::Svc((a >> 5) & 0xffff));
            }
        }
        let Some(&end) = self.functions.get(&target) else {
            return merr(format!(
                "callee has no verified boundary or import at {target:#x}"
            ));
        };
        Ok(Callee::Function(end - target, self.body(target).0))
    }

    pub fn fingerprint(&mut self, start: usize) -> Result<String, MatchError> {
        if let Some(p) = self.prints.get(&start) {
            return p.clone().ok_or(MatchError(format!(
                "function at {start:#x} has no fingerprint"
            )));
        }
        let (body, branches) = self.body(start);
        let mut refs = vec![];
        let mut fail = None;
        for (off, t) in branches {
            match self.callee(t) {
                Ok(c) => refs.push(format!(
                    "[{off},{}]",
                    match c {
                        Callee::Function(size, d) => format!("[\"function\",{size},\"{d}\"]"),
                        Callee::Import(n) => format!("[\"import\",{}]", json_str(&n)),
                        Callee::Svc(n) => format!("[\"svc\",{n}]"),
                    }
                )),
                Err(e) => {
                    fail = Some(e);
                    break;
                }
            }
        }
        if let Some(e) = fail {
            self.prints.insert(start, None);
            return Err(e);
        }
        let json = format!("[\"{body}\",[{}]]", refs.join(","));
        let p = hex(&Sha256::digest(json.as_bytes()));
        self.prints.insert(start, Some(p.clone()));
        Ok(p)
    }

    /// The start of the one function of `size` with one of `prints`.
    pub fn locate(&mut self, size: usize, prints: &[String]) -> Result<usize, MatchError> {
        let starts: Vec<usize> = self
            .functions
            .iter()
            .filter(|(&s, &e)| e - s == size)
            .map(|(&s, _)| s)
            .collect();
        let mut found = vec![];
        for s in starts {
            if let Ok(p) = self.fingerprint(s) {
                if prints.contains(&p) {
                    found.push(s);
                }
            }
        }
        if found.len() != 1 {
            return merr(format!(
                "{} matching functions (need exactly one)",
                found.len()
            ));
        }
        Ok(found[0])
    }

    /// One function's cuts placed: (module offset, original, replacement, reason), the original bytes checked.
    pub fn cuts_at(
        &self,
        start: usize,
        cuts: &[(u64, String, String, String)],
    ) -> Result<Vec<(u64, Vec<u8>, Vec<u8>, String)>, MatchError> {
        let mut out = vec![];
        for (delta, old, new, reason) in cuts {
            let (o, n) = (unhex(old)?, unhex(new)?);
            let off = start + *delta as usize;
            if self.image.get(off..off + o.len()) != Some(&o[..]) {
                return merr(format!("original bytes differ at {off:#x}"));
            }
            out.push((off as u64, o, n, format!("{reason} [function match]")));
        }
        Ok(out)
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Result<Vec<u8>, MatchError> {
    if s.len() % 2 != 0 {
        return merr(format!("odd hex {s}"));
    }
    (0..s.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| MatchError(format!("bad hex {s}")))
        })
        .collect()
}

fn json_str(s: &str) -> String {
    let mut o = String::from("\"");
    for ch in s.chars() {
        match ch {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7e => {
                let mut b = [0u16; 2];
                for u in c.encode_utf16(&mut b) {
                    o.push_str(&format!("\\u{u:04x}"));
                }
            }
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_and_register_classes() {
        assert_eq!(relative(0x9400_0010), Some((0x03ff_ffff, 0x40)), "bl +0x40");
        assert_eq!(relative(0x5400_0040).map(|r| r.1), Some(8), "b.eq +8");
        assert!(is_br_blr_ret(0xd65f_03c0));
        assert!(writes_gpr(0x9100_0421, 1), "add x1, x1, #1");
        assert!(!writes_gpr(0xf900_0021, 1), "str x1, [x1]");
        assert!(writes_gpr(0xf940_0021, 1), "ldr x1, [x1]");
        assert!(writes_gpr(0xaa0203e1, 1), "mov x1, x2");
        assert!(!writes_gpr(0xfa41_1820, 0), "ccmp: flags only");
        assert!(writes_gpr(0x9400_0010, 30), "bl writes lr");
        assert_eq!(json_str("a\"b"), "\"a\\\"b\"");
    }

    /// Against the fingerprints nxnative computed (Python, capstone): Pokémon Legends: Arceus 1.1.1's capture-demo
    /// function (rule size 0xa70 at main+0x16e33ec), matched in the owner's dump when it is on this PC.
    #[test]
    fn arceus_capture_demo_function_matches_nxnative() {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../build/games/arceus/arceus_data/exefs/main");
        let Ok(raw) = std::fs::read(&p) else {
            eprintln!("no Arceus build here: skipped");
            return;
        };
        let nso = crate::loader::nso::Nso::parse(&raw).unwrap();
        let mut img = Image::new(&nso.image, nso.text.0 + nso.text.1).unwrap();
        assert!(
            img.functions.len() > 10_000,
            "{} functions",
            img.functions.len()
        );
        let want = ["123f93c4fa74d68f6100dab6ec8310a5047b2b3535b72361b6e658e3e48d792e".to_string()];
        let start = img.locate(0xa70, &want).unwrap();
        assert_eq!(start, 0x16e341c - 0x30);
    }
}
