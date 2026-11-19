//! What every NVN shader is made of, whatever file holds it: a control section and a code area.
//!
//! - Control section: magic `0x98761234`. NVN (Switch 1) sections are 0x880 bytes (+0x20 their own length when it is
//!   0x760..0x880, +0x700 the c1 segment's start in the code area, +0x704 the code area's length). NVN2 (Switch 2)
//!   sections have version `0xc9` at +4, their own length at +0x30 (0 outside BNSH files), the code area's length at
//!   +0x44 and the stage byte at +0x56 (5 compute, 6 task, 7 mesh, 8 ray tracing).
//! - Code area: NVN graphics starts with a 0x30-byte header (`0x12345678`) and a 0x50-byte SPH; compute may have a
//!   `0x12345679` header or none. NVN2 starts with a 0x80-byte SPH and has no header.
//! - Key: the first 16 hex digits of SHA-256(control bytes + code area), as the runtime computes it when the game
//!   creates a program. Control bytes: an NVN2 section's first 0x48 with +0x30..+0x34 zeroed, else the section's own
//!   length (+0x20) when it is 0x760..0x880, else the first 0x880.

use sha2::{Digest, Sha256};
use std::fmt;

pub const CTL_MAGIC: [u8; 4] = 0x9876_1234u32.to_le_bytes();
pub const GFX_MAGIC: [u8; 4] = 0x1234_5678u32.to_le_bytes();
pub const CS_MAGIC: [u8; 4] = 0x1234_5679u32.to_le_bytes();
pub const NVN2_KEY_CTL: usize = 0x48;

/// A reader's refusal: the file it came from is reported and left out, the scan goes on.
pub type R<T> = Result<T, String>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Stage {
    Vs,
    Hs,
    Ds,
    Gs,
    Ps,
    Cs,
    Ts,
    Ms,
    Rt,
}

impl Stage {
    pub const ALL: [Stage; 9] = [Stage::Vs, Stage::Hs, Stage::Ds, Stage::Gs, Stage::Ps, Stage::Cs, Stage::Ts, Stage::Ms, Stage::Rt];
    /// BNSH's stage pointer order (a program's six pointers from +0x08).
    pub const BNSH: [Stage; 6] = [Stage::Vs, Stage::Hs, Stage::Ds, Stage::Gs, Stage::Ps, Stage::Cs];
    pub fn name(self) -> &'static str {
        match self {
            Stage::Vs => "vs",
            Stage::Hs => "hs",
            Stage::Ds => "ds",
            Stage::Gs => "gs",
            Stage::Ps => "ps",
            Stage::Cs => "cs",
            Stage::Ts => "ts",
            Stage::Ms => "ms",
            Stage::Rt => "rt",
        }
    }
    pub fn parse(s: &str) -> Option<Stage> {
        Stage::ALL.into_iter().find(|st| st.name() == s)
    }
    /// The SPH's shader type (word 0 bits 10..13): 1 vertex, 2 hull, 3 domain, 4 geometry, 5 pixel.
    pub fn from_sph_type(t: u32) -> Option<Stage> {
        match t {
            1 => Some(Stage::Vs),
            2 => Some(Stage::Hs),
            3 => Some(Stage::Ds),
            4 => Some(Stage::Gs),
            5 => Some(Stage::Ps),
            _ => None,
        }
    }
}

impl fmt::Display for Stage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

pub fn u16_at(d: &[u8], o: usize) -> Option<u16> {
    d.get(o..o.checked_add(2)?).map(|s| u16::from_le_bytes([s[0], s[1]]))
}
pub fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    d.get(o..o.checked_add(4)?).map(|s| u32::from_le_bytes(s.try_into().unwrap()))
}
pub fn i32_at(d: &[u8], o: usize) -> Option<i32> {
    u32_at(d, o).map(|v| v as i32)
}
pub fn u64_at(d: &[u8], o: usize) -> Option<u64> {
    d.get(o..o.checked_add(8)?).map(|s| u64::from_le_bytes(s.try_into().unwrap()))
}

fn outside(what: &str, n: usize, at: usize, len: usize) -> String {
    format!("{what}: {n:#x} bytes at {at:#x} lie outside the {len:#x} bytes there are")
}
/// A u32 a header or table points at; refused naming the place when it is not inside `d` (a damaged or cut file).
pub fn need32(d: &[u8], at: usize, what: &str) -> R<u32> {
    u32_at(d, at).ok_or_else(|| outside(what, 4, at, d.len()))
}
pub fn need64(d: &[u8], at: usize, what: &str) -> R<u64> {
    u64_at(d, at).ok_or_else(|| outside(what, 8, at, d.len()))
}
pub fn need16(d: &[u8], at: usize, what: &str) -> R<u16> {
    u16_at(d, at).ok_or_else(|| outside(what, 2, at, d.len()))
}
/// `d[at..at + n]`, refused naming the place when the range is not inside `d`.
pub fn span<'a>(d: &'a [u8], at: u64, n: u64, what: &str) -> R<&'a [u8]> {
    let end = at.checked_add(n).filter(|&e| e <= d.len() as u64).ok_or_else(|| outside(what, n as usize, at as usize, d.len()))?;
    Ok(&d[at as usize..end as usize])
}

/// An NVN2 (Switch 2) control section: the magic and version 0xc9.
pub fn nvn2_ctl(ctl: &[u8]) -> bool {
    ctl.len() >= NVN2_KEY_CTL && ctl[..4] == CTL_MAGIC && ctl[4..8] == [0xc9, 0, 0, 0]
}

/// Control bytes the key covers.
pub fn ctl_len(ctl: &[u8]) -> usize {
    if nvn2_ctl(ctl) {
        return NVN2_KEY_CTL;
    }
    match u32_at(ctl, 0x20) {
        Some(n) if (0x760..0x880).contains(&n) => n as usize,
        _ => 0x880,
    }
}

/// The control bytes the key hashes.
pub fn key_ctl(ctl: &[u8]) -> Vec<u8> {
    let n = ctl_len(ctl).min(ctl.len());
    let mut c = ctl[..n].to_vec();
    if nvn2_ctl(ctl) {
        c[0x30..0x34].fill(0);
    }
    c
}

/// The runtime's shader key: 16 lowercase hex digits.
pub fn skey(ctl: &[u8], code: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(key_ctl(ctl));
    h.update(code);
    let d = h.finalize();
    d[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// Two copies of one key hold the same shader: equal code areas, and equal control sections up to their own length.
/// (Pokémon Legends: Z-A's layout archives give control sections whose trailing bytes differ between copies.)
pub fn same_shader(a: (&[u8], &[u8]), b: (&[u8], &[u8])) -> bool {
    let ((ca, ka), (cb, kb)) = (a, b);
    if ka != kb {
        return false;
    }
    if nvn2_ctl(ca) {
        return key_ctl(ca) == key_ctl(cb);
    }
    let own = u32_at(ca, 0x20).map(|v| v as usize).unwrap_or(ca.len());
    let n = if own >= 0x760 && own <= ca.len().min(cb.len()) { own } else { ca.len().max(cb.len()) };
    ca.get(..n) == cb.get(..n)
}

/// A graphics code area's stage from its SPH word 0 (after the 0x30 header): shader type in bits 10..13. A graphics
/// header's type 0 with SPH type 1 is a vertex shader (Catherine: Full Body's GLSLC writes it so).
pub fn sph_stage(code: &[u8]) -> Option<Stage> {
    let w = u32_at(code, 0x30)?;
    if (w >> 10) & 0xf == 0 && w & 0x1f == 1 {
        return Some(Stage::Vs);
    }
    Stage::from_sph_type((w >> 10) & 0xf)
}

/// An NVN2 shader's stage: compute, task, mesh and ray tracing by the control's stage byte; graphics by the SPH that
/// starts the code area.
pub fn nvn2_stage(ctl: &[u8], code: &[u8]) -> Option<Stage> {
    match ctl.get(0x56) {
        Some(5) => return Some(Stage::Cs),
        Some(6) => return Some(Stage::Ts),
        Some(7) => return Some(Stage::Ms),
        Some(8) => return Some(Stage::Rt),
        _ => {}
    }
    let w = u32_at(code, 0)?;
    if (w >> 10) & 0xf == 0 && w & 0x1f == 1 {
        return Some(Stage::Vs);
    }
    Stage::from_sph_type((w >> 10) & 0xf)
}

/// A control section and code area that belong together, for `st`; the reason when not.
pub fn check(st: Stage, ctl: &[u8], code: &[u8]) -> Option<&'static str> {
    if ctl.len() < 0x24 || ctl[..4] != CTL_MAGIC || ctl.len() < ctl_len(ctl) {
        return Some("bad control section");
    }
    if st == Stage::Rt {
        if !nvn2_ctl(ctl) || ctl.len() < 0x68 || ctl[0x56] != 8 {
            return Some("bad NVN2 ray-tracing control section");
        }
        let (nmeta, pmeta, ncode) = (u32_at(ctl, 0x5c).unwrap() as usize, u32_at(ctl, 0x60).unwrap() as usize, u32_at(ctl, 0x64).unwrap() as usize);
        if ncode != code.len() {
            return Some("RT control +0x64 does not match the code area length");
        }
        if nmeta == 0 || pmeta < 0x68 || pmeta % 8 != 0 || pmeta > ctl.len() || nmeta > ctl.len() - pmeta {
            return Some("RT metadata range lies outside the control section");
        }
    }
    if nvn2_ctl(ctl) {
        return if u32_at(ctl, 0x44) == Some(code.len() as u32) { None } else { Some("control +0x44 does not match the code area length") };
    }
    let head = code.get(..4);
    let cs_bad = st == Stage::Cs && (head == Some(&CS_MAGIC[..]) || head == Some(&GFX_MAGIC[..])) && head != Some(&CS_MAGIC[..]);
    if cs_bad || (st != Stage::Cs && head != Some(&GFX_MAGIC[..])) {
        return Some("code area header magic does not match the stage");
    }
    if u32_at(ctl, 0x704) != Some(code.len() as u32) {
        return Some("control +0x704 does not match the code area length");
    }
    None
}

/// One shader as a reader found it.
#[derive(Debug, Clone)]
pub struct Found {
    /// Where: the file, the container path inside it and the reader's tag (`/Shader/x.bfsha#BNSH@0x0#var3`).
    pub label: String,
    pub stage: Stage,
    pub ctl: Vec<u8>,
    pub code: Vec<u8>,
    /// The program variation it belongs to, when the format says (vertex/mesh + pixel pairs come from it).
    pub group: Option<String>,
}

impl Found {
    pub fn new(label: String, stage: Stage, ctl: &[u8], code: &[u8]) -> Found {
        Found { label, stage, ctl: ctl.to_vec(), code: code.to_vec(), group: None }
    }
    pub fn grouped(mut self, g: impl Into<String>) -> Found {
        self.group = Some(g.into());
        self
    }
    pub fn key(&self) -> String {
        skey(&self.ctl, &self.code)
    }
}

#[cfg(test)]
pub(crate) mod synth {
    //! Small synthetic control sections and code areas for the format tests.
    use super::*;

    /// An NVN (Switch 1) pixel shader: a 0x880-byte control section and a 0x30 header + 0x50 SPH + 32 bytes of code.
    pub fn nvn1_ps(seed: u8) -> (Vec<u8>, Vec<u8>) {
        let mut code = vec![0u8; 0x30 + 0x50 + 0x20];
        code[..4].copy_from_slice(&GFX_MAGIC);
        code[0x30..0x34].copy_from_slice(&((5u32 << 10) | 2).to_le_bytes());
        code[0x90] = seed;
        let mut ctl = vec![0u8; 0x880];
        ctl[..4].copy_from_slice(&CTL_MAGIC);
        ctl[4] = 1;
        ctl[0x704..0x708].copy_from_slice(&(code.len() as u32).to_le_bytes());
        ctl[0x100] = seed;
        (ctl, code)
    }
    /// An NVN (Switch 1) vertex shader.
    pub fn nvn1_vs(seed: u8) -> (Vec<u8>, Vec<u8>) {
        let (ctl, mut code) = nvn1_ps(seed);
        code[0x30..0x34].copy_from_slice(&((1u32 << 10) | 1).to_le_bytes());
        (ctl, code)
    }
    /// An NVN2 (Switch 2) shader: a 0xd8-byte control section; graphics stages by the SPH, compute by +0x56 = 5.
    pub fn nvn2(stage: Stage, seed: u8) -> (Vec<u8>, Vec<u8>) {
        let mut code = vec![0u8; 0x80 + 0x40 + 0x100];
        let t = match stage {
            Stage::Vs => 1,
            Stage::Ps => 5,
            _ => 1,
        };
        code[..4].copy_from_slice(&((t << 10) | 4u32).to_le_bytes());
        code[0x90] = seed;
        let mut ctl = vec![0u8; 0xd8];
        ctl[..4].copy_from_slice(&CTL_MAGIC);
        ctl[4] = 0xc9;
        ctl[0x30..0x34].copy_from_slice(&0xd8u32.to_le_bytes());
        ctl[0x38..0x3c].copy_from_slice(&0xc0u32.to_le_bytes());
        ctl[0x40..0x44].copy_from_slice(&0x100u32.to_le_bytes());
        ctl[0x44..0x48].copy_from_slice(&(code.len() as u32).to_le_bytes());
        if stage == Stage::Cs {
            ctl[0x56] = 5;
            ctl[0x58..0x5c].copy_from_slice(&32u32.to_le_bytes());
            ctl[0x5c..0x60].copy_from_slice(&1u32.to_le_bytes());
            ctl[0x60..0x64].copy_from_slice(&1u32.to_le_bytes());
        }
        ctl[0xa0] = seed;
        (ctl, code)
    }
}

#[cfg(test)]
mod tests {
    use super::synth::*;
    use super::*;

    #[test]
    fn key_rules() {
        let (ctl, code) = nvn1_ps(1);
        let mut h = Sha256::new();
        h.update(&ctl[..0x880]);
        h.update(&code);
        assert_eq!(skey(&ctl, &code), h.finalize()[..8].iter().map(|b| format!("{b:02x}")).collect::<String>());
        // NVN2: the first 0x48 bytes, +0x30 zeroed; bytes past 0x48 do not count
        let (mut c2, k2) = nvn2(Stage::Vs, 1);
        let a = skey(&c2, &k2);
        c2[0x30] = 0x77;
        c2[0xa0] = 0x55;
        assert_eq!(a, skey(&c2, &k2));
        c2[0x10] = 1;
        assert_ne!(a, skey(&c2, &k2));
        // own length 0x850: bytes past it do not count
        let (mut c3, k3) = nvn1_ps(2);
        c3[0x20..0x24].copy_from_slice(&0x850u32.to_le_bytes());
        let b = skey(&c3, &k3);
        c3[0x860] = 9;
        assert_eq!(b, skey(&c3, &k3));
    }

    #[test]
    fn stages_and_checks() {
        let (c, k) = nvn1_ps(0);
        assert_eq!(sph_stage(&k), Some(Stage::Ps));
        assert_eq!(check(Stage::Ps, &c, &k), None);
        assert!(check(Stage::Cs, &c, &k).is_some());
        let (c, k) = nvn2(Stage::Cs, 0);
        assert_eq!(nvn2_stage(&c, &k), Some(Stage::Cs));
        assert_eq!(check(Stage::Cs, &c, &k), None);
        assert!(check(Stage::Cs, &c, &k[1..]).is_some());
        let (c, k) = nvn2(Stage::Ps, 0);
        assert_eq!(nvn2_stage(&c, &k), Some(Stage::Ps));
    }
}
