//! GLSLC compiler outputs (magic `0x19866891`): the shaders games compile offline into their code or their files.
//! An output holds section headers; each GPU code section names its control section and code area. NVN2 outputs
//! (GPU code version 0xc9 at +0x30) keep the same fields further along.

use crate::nvn::*;

pub const MAGIC: [u8; 4] = [0x91, 0x68, 0x86, 0x19];

/// A GLSLC output's stage: NVN2 by its control section; else by the code header (graphics: the SPH), or compute when
/// the code has no header and the control's thread group (+0x748) is filled.
pub fn stage(ctl: &[u8], code: &[u8]) -> Option<Stage> {
    if nvn2_ctl(ctl) {
        return nvn2_stage(ctl, code);
    }
    let head = code.get(..4);
    let headerless_cs = head != Some(&CS_MAGIC[..]) && head != Some(&GFX_MAGIC[..]) && ctl.len() >= 0x754 && (0..3).all(|i| u32_at(ctl, 0x748 + 4 * i) != Some(0));
    if head == Some(&CS_MAGIC[..]) || headerless_cs {
        Some(Stage::Cs)
    } else if head == Some(&GFX_MAGIC[..]) && code.len() >= 0x34 {
        sph_stage(code)
    } else {
        None
    }
}

/// (the output's length, section count, first section header, distance between section headers), relative to `h`.
pub(crate) fn layout(d: &[u8], h: usize) -> (usize, usize, usize, usize) {
    let w = |o: usize| u32_at(d, h + o).unwrap_or(0) as usize;
    if h + 0x68 <= d.len() && w(0x30) == 0xc9 && w(0x44) == 0 {
        return (w(0x5c), w(0x64), 0xa8, 0x98);
    }
    if h + 0x50 <= d.len() {
        (w(0x44), w(0x4c), 0x90, 0x90)
    } else {
        (0, 0, 0x90, 0x90)
    }
}

/// The GPU code sections of the output at `h` by its section headers: [(control, code)], or None.
fn sections(d: &[u8], h: usize, size: usize) -> Option<Vec<(&[u8], &[u8])>> {
    let (_, n, first, stride) = layout(d, h);
    if !(1..=16).contains(&n) || h + first + stride * n > d.len() {
        return None;
    }
    let mut secs = vec![];
    for i in 0..n {
        let s = h + first + stride * i;
        let (ssz, soff, typ) = (u32_at(d, s)? as usize, u32_at(d, s + 4)? as usize, u32_at(d, s + 8)?);
        if typ != 0 {
            continue;
        }
        let (co, ko, kn, cn) = (u32_at(d, s + 0x30)? as usize, u32_at(d, s + 0x34)? as usize, u32_at(d, s + 0x38)? as usize, u32_at(d, s + 0x3c)? as usize);
        if soff + ssz > size || co + cn > ssz || ko + kn > ssz {
            return None;
        }
        let b = h + soff;
        secs.push((d.get(b + co..b + co + cn)?, d.get(b + ko..b + ko + kn)?));
    }
    Some(secs)
}

/// An output of several stages compiled as one program: every GPU code section; [] when one fails the check or two
/// share a stage.
fn program(d: &[u8], h: usize, size: usize, label: &str) -> Vec<Found> {
    let secs = sections(d, h, size).unwrap_or_default();
    let sts: Vec<Option<Stage>> = secs.iter().map(|(c, k)| stage(c, k)).collect();
    let mut uniq = sts.clone();
    uniq.sort();
    uniq.dedup();
    if secs.len() < 2 || uniq.len() < sts.len() {
        return vec![];
    }
    if secs.iter().zip(&sts).any(|((c, k), st)| st.is_none_or(|st| check(st, c, k).is_some())) {
        return vec![];
    }
    let tag = format!("{label}#GLSLC@{h:#x}#var");
    secs.iter().zip(&sts).map(|((c, k), st)| Found::new(tag.clone(), st.unwrap(), c, k).grouped(tag.clone())).collect()
}

/// Every GLSLC output in `d` that passes the checks.
pub fn shaders(d: &[u8], label: &str) -> Vec<Found> {
    shaders_skip(d, label, &[])
}

/// The same, leaving out outputs whose magic lies in one of `skip`'s byte ranges (compressed streams a reader decodes
/// and reads on its own).
pub fn shaders_skip(d: &[u8], label: &str, skip: &[(usize, usize)]) -> Vec<Found> {
    let mut out = vec![];
    for h in memchr::memmem::find_iter(d, &MAGIC) {
        if h + 0x48 > d.len() || skip.iter().any(|&(a, b)| a <= h && h < b) {
            continue;
        }
        let (size, _, first, _) = layout(d, h);
        if first == 0xa8 {
            if !(size > 0 && size <= d.len() - h) {
                continue;
            }
            let secs = sections(d, h, size).unwrap_or_default();
            if secs.len() > 1 {
                out.extend(program(d, h, size, label));
            } else if let Some(&(c, k)) = secs.first() {
                if let Some(st) = stage(c, k).filter(|&st| check(st, c, k).is_none()) {
                    out.push(Found::new(format!("{label}#GLSLC@{h:#x}"), st, c, k));
                }
            }
            continue;
        }
        if !(size > 0x760 && size <= d.len() - h) {
            continue;
        }
        let Some(c) = memchr::memmem::find(&d[h..h + size], &CTL_MAGIC).map(|c| c + h) else { continue };
        if memchr::memmem::find(&d[c + 4..h + size], &CTL_MAGIC).is_some() {
            out.extend(program(d, h, size, label));
            continue;
        }
        if let Some(sec) = sections(d, h, size) {
            if sec.len() == 1 {
                let (cc, kk) = sec[0];
                if let Some(st) = stage(cc, kk).filter(|&st| check(st, cc, kk).is_none()) {
                    out.push(Found::new(format!("{label}#GLSLC@{h:#x}"), st, cc, kk));
                    continue;
                }
            }
        }
        // no usable section header: the code area right after the control section's +0x20 length
        let (nctl, ncode) = if c + 0x708 <= h + size { (u32_at(d, c + 0x20).unwrap() as usize, u32_at(d, c + 0x704).unwrap() as usize) } else { (0, 0) };
        if !(nctl >= 0x760 && nctl & 3 == 0 && c + nctl + ncode <= h + size) {
            continue;
        }
        let (ctl, code) = (&d[c..c + nctl], &d[c + nctl..c + nctl + ncode]);
        if let Some(st) = stage(ctl, code).filter(|&st| check(st, ctl, code).is_none()) {
            out.push(Found::new(format!("{label}#GLSLC@{h:#x}"), st, ctl, code));
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::nvn::synth::*;

    /// A GLSLC output of one stage with its section header.
    pub fn make_output(ctl: &[u8], code: &[u8]) -> Vec<u8> {
        let mut d = vec![0u8; 0x200];
        d[..4].copy_from_slice(&MAGIC);
        let sec = 0x90;
        let data = 0x200usize;
        d.extend_from_slice(ctl);
        d.extend_from_slice(code);
        let size = d.len();
        d[0x44..0x48].copy_from_slice(&(size as u32).to_le_bytes());
        d[0x4c..0x50].copy_from_slice(&1u32.to_le_bytes());
        d[sec..sec + 4].copy_from_slice(&((size - data) as u32).to_le_bytes());
        d[sec + 4..sec + 8].copy_from_slice(&(data as u32).to_le_bytes());
        d[sec + 0x30..sec + 0x34].copy_from_slice(&0u32.to_le_bytes());
        d[sec + 0x34..sec + 0x38].copy_from_slice(&(ctl.len() as u32).to_le_bytes());
        d[sec + 0x38..sec + 0x3c].copy_from_slice(&(code.len() as u32).to_le_bytes());
        d[sec + 0x3c..sec + 0x40].copy_from_slice(&(ctl.len() as u32).to_le_bytes());
        d
    }

    #[test]
    fn one_stage_output() {
        let (c, k) = nvn1_ps(7);
        let mut d = vec![1u8; 0x30];
        d.extend(make_output(&c, &k));
        let out = shaders(&d, "main");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].stage, Stage::Ps);
        assert_eq!(out[0].label, "main#GLSLC@0x30");
        assert_eq!(out[0].key(), skey(&c, &k));
    }
}
