//! Patch tables as data (`games/<id>/patches.toml`, `shared/*.toml`, and the owner's `games/<id>/turbo.toml`).
//!
//! The exporter (`tools/export.py`) turns nxnative's `patches.py` into this; see its header for how stubs become
//! words plus relocations. Offsets are module offsets (the module image starts at 0), as the rewriter's.
//!
//! | Table | What | Keyed by |
//! |---|---|---|
//! | `cut` | original bytes → replacement bytes at an offset, asserted first | a module's build id |
//! | `obs` | an observation gate (nxnative host callback number) at an instruction | build id |
//! | `capability` | launcher features a build's stubs provide | build id |
//! | `binding` | why each patch is bound the way it is (documentation) | — |
//! | `stub` | code assembled into the patch area (`code` + `relocs`), the sites it checks and the words it puts | build id |
//! | `match` | the same, placed by a function fingerprint (size + sha256) on builds without an entry of their own | module |

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchFile {
    pub source: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cut: Vec<Cut>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub obs: Vec<Obs>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capability: Vec<Capability>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub binding: Vec<Binding>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stub: Vec<Stub>,
    #[serde(default, rename = "match", skip_serializing_if = "Vec::is_empty")]
    pub matches: Vec<MatchRule>,
}

/// Replace `original` (hex bytes) with `replacement` at `offset` of the module whose build id is `build`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cut {
    pub build: String,
    pub offset: u64,
    pub original: String,
    pub replacement: String,
    pub reason: String,
    /// The module (nxnative keys a cut by build id alone; Turbo's entries name it)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
    /// `build`: nxrs applies it when it builds the game (the translator's patch table). Absent: data for the lanes
    /// that port nxnative's rewriter (it was made for code running natively on Apple's kernel).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apply: Option<String>,
    /// The patch set's name in the translator's table, and the environment variable that turns it off at run time
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Obs {
    pub build: String,
    pub offset: u64,
    pub number: i64,
    pub original_word: u32,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capability {
    pub build: String,
    pub names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub what: String,
    pub now: String,
    pub goal: String,
    pub purpose: String,
    pub why: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kept: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked: Option<String>,
}

/// A pc-relative word: its immediate is filled in for `target` + `addend` where it is placed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reloc {
    pub index: usize,
    /// b, bl, bcond, cbz, tbz, adrp, adr
    pub op: String,
    /// module (offset 0), area (the stub's code), state (the frame-step state page), base (the matched function),
    /// other:<label> (another matched function)
    pub target: String,
    pub addend: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Site {
    pub base: String,
    pub offset: i64,
    pub original: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Put {
    pub base: String,
    pub offset: i64,
    pub words: Vec<u32>,
    pub reason: String,
    #[serde(default)]
    pub relocs: Vec<Reloc>,
}

/// The recorded body of a stub. `evaluated = false`: it reads the module's own code (or an anchored rule's find),
/// so it can only be assembled against the game; `needs` says what.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StubBody {
    pub evaluated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub code: Vec<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relocs: Vec<Reloc>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sites: Vec<Site>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub puts: Vec<Put>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_max: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub render_scale: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fps_ready: Option<bool>,
}

/// FRAME_STEP / STUBS of one build. (The body is flattened, and serde checks unknown fields there.)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stub {
    /// frame_step or stub
    pub kind: String,
    pub build: String,
    #[serde(flatten)]
    pub body: StubBody,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MatchCut {
    pub delta: u64,
    pub original: String,
    pub replacement: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MatchFunction {
    pub size: u64,
    pub fingerprints: Vec<String>,
    #[serde(default)]
    pub cuts: Vec<MatchCut>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Other {
    pub label: String,
    pub size: u64,
    pub fingerprints: Vec<String>,
}

/// A MATCH_CUTS / MATCH_FRAME_STEP / MATCH_STUBS rule: the function it patches is found by size and fingerprint
/// (or, anchored, as the one caller of an import), on builds that have no entry of their own.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MatchRule {
    /// cuts, frame_step or stubs
    pub kind: String,
    pub module: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fingerprints: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calls: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fps_required: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reviewed: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cuts: Vec<MatchCut>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub functions: Vec<MatchFunction>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub others: Vec<Other>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stub: Option<StubBody>,
}

impl PatchFile {
    pub fn parse(text: &str) -> Result<PatchFile, String> {
        toml::from_str(text).map_err(|e| e.to_string())
    }

    pub fn to_toml(&self) -> Result<String, String> {
        toml::to_string(self).map_err(|e| e.to_string())
    }
}

/// Whether a table's build key names a module's build id: both as lowercase hex, equal over the shorter of the
/// two, which must be at least 16 bytes (nxnative keys 16 bytes, Turbo 20, a module's id runs to 32).
pub fn build_matches(key: &str, build_id: &str) -> bool {
    let (k, b) = (key.to_ascii_lowercase(), build_id.to_ascii_lowercase());
    let n = k.len().min(b.len());
    n >= 32 && k[..n] == b[..n]
}

fn sext(v: u32, bits: u32) -> i64 {
    let v = v as i64;
    if v & (1 << (bits - 1)) != 0 { v - (1 << bits) } else { v }
}

/// The target of a pc-relative word at `pc`.
pub fn reloc_target(op: &str, w: u32, pc: u64) -> Option<u64> {
    let pc = pc as i64;
    Some(match op {
        "b" | "bl" => pc + 4 * sext(w & 0x3FF_FFFF, 26),
        "bcond" | "cbz" => pc + 4 * sext((w >> 5) & 0x7FFFF, 19),
        "tbz" => pc + 4 * sext((w >> 5) & 0x3FFF, 14),
        "adrp" | "adr" => {
            let imm = sext(((w >> 5) & 0x7FFFF) << 2 | (w >> 29) & 3, 21);
            if op == "adrp" { (pc & !0xFFF) + (imm << 12) } else { pc + imm }
        }
        _ => return None,
    } as u64)
}

/// `w` with its immediate set so that, placed at `pc`, it reaches `target`; None when out of reach.
pub fn reloc_encode(op: &str, w: u32, pc: u64, target: u64) -> Option<u32> {
    let (pc, t) = (pc as i64, target as i64);
    let fits = |d: i64, bits: u32| (-(1i64 << (bits - 1))..(1i64 << (bits - 1))).contains(&d);
    match op {
        "b" | "bl" => {
            let d = (t - pc) >> 2;
            fits(d, 26).then(|| (w & !0x3FF_FFFF) | (d as u32 & 0x3FF_FFFF))
        }
        "bcond" | "cbz" => {
            let d = (t - pc) >> 2;
            fits(d, 19).then(|| (w & !(0x7FFFF << 5)) | ((d as u32 & 0x7FFFF) << 5))
        }
        "tbz" => {
            let d = (t - pc) >> 2;
            fits(d, 14).then(|| (w & !(0x3FFF << 5)) | ((d as u32 & 0x3FFF) << 5))
        }
        "adrp" | "adr" => {
            let d = if op == "adrp" { ((t & !0xFFF) - (pc & !0xFFF)) >> 12 } else { t - pc };
            fits(d, 21).then(|| (w & !(0x6000_0000 | 0x7FFFF << 5)) | ((d as u32 & 3) << 29) | (((d as u32 >> 2) & 0x7FFFF) << 5))
        }
        _ => None,
    }
}

/// Where a stub's symbols are, as module offsets.
pub struct Layout<'a> {
    pub area: u64,
    pub state: u64,
    pub base: u64,
    pub others: &'a [(&'a str, u64)],
}

impl Layout<'_> {
    pub fn symbol(&self, name: &str) -> Option<u64> {
        match name {
            "module" => Some(0),
            "area" => Some(self.area),
            "state" => Some(self.state),
            "base" => Some(self.base),
            _ => name.strip_prefix("other:").and_then(|l| self.others.iter().find(|(k, _)| *k == l).map(|(_, v)| *v)),
        }
    }
}

/// A stub placed at a layout: its code (at `area`), the site offsets it checks, and the (offset, words) it puts.
#[derive(Debug, PartialEq)]
pub struct Placed {
    pub code: Vec<u32>,
    pub sites: Vec<u64>,
    pub puts: Vec<(u64, Vec<u32>)>,
}

impl StubBody {
    pub fn place(&self, at: &Layout) -> Result<Placed, String> {
        if !self.evaluated {
            return Err(format!("not recorded: needs {}", self.needs.as_deref().unwrap_or("the game")));
        }
        let sym = |n: &str| at.symbol(n).ok_or_else(|| format!("no symbol {n} in the layout"));
        let apply = |words: &[u32], pc0: u64, relocs: &[Reloc]| -> Result<Vec<u32>, String> {
            let mut w = words.to_vec();
            for r in relocs {
                let pc = pc0 + 4 * r.index as u64;
                let t = (sym(&r.target)? as i64 + r.addend) as u64;
                w[r.index] = reloc_encode(&r.op, w[r.index], pc, t).ok_or_else(|| format!("word {}: {} out of reach", r.index, r.op))?;
            }
            Ok(w)
        };
        let code = apply(&self.code, at.area, &self.relocs)?;
        let sites = self.sites.iter().map(|s| Ok((sym(&s.base)? as i64 + s.offset) as u64)).collect::<Result<_, String>>()?;
        let mut puts = vec![];
        for p in &self.puts {
            let off = (sym(&p.base)? as i64 + p.offset) as u64;
            puts.push((off, apply(&p.words, off, &p.relocs)?));
        }
        Ok(Placed { code, sites, puts })
    }
}

/// `hex` bytes as little-endian words (a cut's bytes, when they are whole words).
pub fn words_of(hex: &str) -> Option<Vec<u32>> {
    if hex.len() % 8 != 0 {
        return None;
    }
    (0..hex.len() / 8)
        .map(|i| {
            let b = u32::from_str_radix(&hex[8 * i..8 * i + 8], 16).ok()?;
            Some(b.swap_bytes())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_inverts_target() {
        for (op, w) in [("b", 0x1400_0000u32), ("bl", 0x9400_0000), ("bcond", 0x5400_0000), ("cbz", 0x3400_0001), ("tbz", 0x3600_0002), ("adrp", 0x9000_0010), ("adr", 0x1000_0003)] {
            for (pc, t) in [(0x1000u64, 0x2000u64), (0x80_0000, 0x7F_F000), (0x10_0004, 0x10_0000)] {
                let e = reloc_encode(op, w, pc, t).unwrap();
                let back = reloc_target(op, e, pc).unwrap();
                let want = if op == "adrp" { t & !0xFFF } else { t };
                assert_eq!(back, want, "{op} {pc:#x} -> {t:#x}");
                assert_eq!(e & !match op { "b" | "bl" => 0x3FF_FFFF, "bcond" | "cbz" => 0x7FFFF << 5, "tbz" => 0x3FFF << 5, _ => 0x6000_0000 | 0x7FFFF << 5 }, w & !match op { "b" | "bl" => 0x3FF_FFFF, "bcond" | "cbz" => 0x7FFFF << 5, "tbz" => 0x3FFF << 5, _ => 0x6000_0000 | 0x7FFFF << 5 });
            }
        }
        assert_eq!(reloc_encode("tbz", 0x3600_0000, 0, 0x10_0000), None, "tbz reaches 32 KB");
    }

    #[test]
    fn build_keys() {
        assert!(build_matches("b1f12fd919eae86ab8a978317677e64b", "B1F12FD919EAE86AB8A978317677E64BCE443D1F00"));
        assert!(build_matches("B1F12FD919EAE86AB8A978317677E64BCE443D1F", "b1f12fd919eae86ab8a978317677e64bce443d1f"));
        assert!(!build_matches("b1f12fd919eae86ab8a978317677e64bce443d1f", "b1f12fd919eae86ab8a978317677e64bce443d1e"));
        assert!(!build_matches("b1f12fd9", "b1f12fd919eae86ab8a978317677e64b"), "a key under 16 bytes names nothing");
    }

    #[test]
    fn cut_bytes_as_words() {
        assert_eq!(words_of("3481c039"), Some(vec![0x39C0_8134]));
        assert_eq!(words_of("e9030232"), Some(vec![0x3202_03e9]));
        assert_eq!(words_of("e903"), None);
    }
}
