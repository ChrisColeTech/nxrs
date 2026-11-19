//! Loading the game's modules into guest memory: NSO (ExeFS `main`, `subsdk*`, `sdk`) and NRO (homebrew) images
//! mapped at the bases the translator chose (its layout.json / layout.h), relocated, imports bound by name.
//!
//! Relocations: RELATIVE, ABS64, GLOB_DAT, JUMP_SLOT and packed RELR; imports resolve across modules in load order,
//! with the `HOST_OVERRIDE` rule on top: a host function registered under a symbol's name replaces that symbol in
//! every GOT/GLOB_DAT/ABS64 of every module, also where a module defines it. An import nothing provides binds to a stub
//! that names it when called (an UNIMPL report) instead of failing the load.

use crate::mem::{self, Space, ST_CODE, ST_CODEDATA};
use std::collections::HashMap;

const R_AARCH64_NONE: u32 = 0;
const R_AARCH64_ABS64: u32 = 257;
const R_AARCH64_GLOB_DAT: u32 = 1025;
const R_AARCH64_JUMP_SLOT: u32 = 1026;
const R_AARCH64_RELATIVE: u32 = 1027;

const DT_NULL: u64 = 0;
const DT_HASH: u64 = 4;
const DT_STRTAB: u64 = 5;
const DT_SYMTAB: u64 = 6;
const DT_RELA: u64 = 7;
const DT_RELASZ: u64 = 8;
const DT_PLTRELSZ: u64 = 2;
const DT_JMPREL: u64 = 23;
const DT_INIT_ARRAY: u64 = 25;
const DT_INIT_ARRAYSZ: u64 = 27;
const DT_RELRSZ: u64 = 0x23;
const DT_RELR: u64 = 0x24;
const DT_GNU_HASH: u64 = 0x6FFF_FEF5;

/// A module image as it lies in memory (offsets from its base), before relocation.
#[derive(Debug, Clone)]
pub struct Image {
    pub text: (u64, u64),
    pub rodata: (u64, u64),
    pub data: (u64, u64),
    pub bss: u64,
    /// The memory image, page-aligned, bss included (zeros).
    pub bytes: Vec<u8>,
    pub build_id: [u8; 0x20],
}

#[derive(Debug, Clone)]
pub struct Sym {
    pub name: String,
    pub info: u8,
    pub shndx: u16,
    pub value: u64,
}

impl Sym {
    pub fn defined(&self) -> bool {
        self.shndx != 0 && (self.info >> 4) != 0 // not undefined, not local
    }
    pub fn weak(&self) -> bool {
        self.info >> 4 == 2
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Reloc {
    pub off: u64,
    pub typ: u32,
    pub sym: u32,
    pub addend: i64,
}

fn rd32(b: &[u8], o: u64) -> Result<u32, String> {
    b.get(o as usize..o as usize + 4).map(|s| u32::from_le_bytes(s.try_into().unwrap())).ok_or(format!("read past the end at {o:#x}"))
}
fn rd64(b: &[u8], o: u64) -> Result<u64, String> {
    b.get(o as usize..o as usize + 8).map(|s| u64::from_le_bytes(s.try_into().unwrap())).ok_or(format!("read past the end at {o:#x}"))
}

/// Parses an NSO or an NRO file into its memory image.
pub fn parse(raw: &[u8]) -> Result<Image, String> {
    if raw.get(..4) == Some(b"NSO0") {
        let flags = rd32(raw, 0xC)?;
        let mut segs = vec![];
        for i in 0..3u64 {
            let file_off = rd32(raw, 0x10 + i * 0x10)? as usize;
            let mem_off = rd32(raw, 0x14 + i * 0x10)? as u64;
            let size = rd32(raw, 0x18 + i * 0x10)? as usize;
            let csize = rd32(raw, 0x60 + i * 4)? as usize;
            let src = raw.get(file_off..file_off + csize).ok_or(format!("NSO segment {i} truncated"))?;
            let data = if flags & (1 << i) != 0 {
                lz4_flex::block::decompress(src, size).map_err(|e| format!("NSO segment {i}: {e}"))?
            } else {
                src.to_vec()
            };
            if data.len() != size {
                return Err(format!("NSO segment {i}: {} bytes, header says {size}", data.len()));
            }
            segs.push((mem_off, data));
        }
        let bss = rd32(raw, 0x3C)? as u64;
        let end = segs[2].0 + segs[2].1.len() as u64 + bss;
        let mut bytes = vec![0u8; mem::align_up(end, mem::PAGE) as usize];
        for (off, d) in &segs {
            bytes[*off as usize..*off as usize + d.len()].copy_from_slice(d);
        }
        let mut build_id = [0u8; 0x20];
        build_id.copy_from_slice(&raw[0x40..0x60]);
        return Ok(Image {
            text: (segs[0].0, segs[0].1.len() as u64),
            rodata: (segs[1].0, segs[1].1.len() as u64),
            data: (segs[2].0, segs[2].1.len() as u64),
            bss,
            bytes,
            build_id,
        });
    }
    if raw.get(0x10..0x14) == Some(b"NRO0") {
        let seg = |i: u64| -> Result<(u64, u64), String> { Ok((rd32(raw, 0x20 + i * 8)? as u64, rd32(raw, 0x24 + i * 8)? as u64)) };
        let (text, rodata, data) = (seg(0)?, seg(1)?, seg(2)?);
        let bss = rd32(raw, 0x38)? as u64;
        let file_end = data.0 + data.1;
        if raw.len() < file_end as usize {
            return Err("NRO truncated".into());
        }
        let mut bytes = vec![0u8; mem::align_up(file_end + bss, mem::PAGE) as usize];
        for (o, n) in [text, rodata, data] {
            bytes[o as usize..(o + n) as usize].copy_from_slice(&raw[o as usize..(o + n) as usize]);
        }
        let mut build_id = [0u8; 0x20];
        build_id.copy_from_slice(&raw[0x40..0x60]);
        return Ok(Image { text, rodata, data, bss, bytes, build_id });
    }
    Err("neither an NSO nor an NRO".into())
}

/// The dynamic information of an image (MOD0 -> .dynamic), offsets relative to the base.
#[derive(Debug, Clone, Default)]
pub struct Dynamic {
    pub mod0: u64,
    pub dynamic: u64,
    pub bss_start: u64,
    pub bss_end: u64,
    pub eh_frame_hdr: u64,
    pub syms: Vec<Sym>,
    pub relocs: Vec<Reloc>,
    pub init_array: (u64, u64),
}

pub fn dynamic(img: &[u8]) -> Result<Dynamic, String> {
    let mod0 = rd32(img, 4)? as u64;
    if img.get(mod0 as usize..mod0 as usize + 4) != Some(b"MOD0") {
        return Err("no MOD0 header".into());
    }
    let rel = |i: u64| -> Result<u64, String> { Ok((mod0 as i64 + rd32(img, mod0 + 4 + 4 * i)? as i32 as i64) as u64) };
    let mut d = Dynamic { mod0, dynamic: rel(0)?, bss_start: rel(1)?, bss_end: rel(2)?, eh_frame_hdr: rel(3)?, ..Default::default() };
    let mut tags: HashMap<u64, u64> = HashMap::new();
    let mut o = d.dynamic;
    loop {
        let (tag, val) = (rd64(img, o)?, rd64(img, o + 8)?);
        o += 16;
        if tag == DT_NULL {
            break;
        }
        tags.entry(tag).or_insert(val);
    }
    let strtab = tags.get(&DT_STRTAB).copied().unwrap_or(0);
    let cstr = |off: u64| -> String {
        let s = &img[(off as usize).min(img.len())..];
        String::from_utf8_lossy(&s[..s.iter().position(|&b| b == 0).unwrap_or(s.len())]).into_owned()
    };
    let nsyms = if let Some(&h) = tags.get(&DT_HASH) {
        rd32(img, h + 4)? as u64
    } else if let Some(&h) = tags.get(&DT_GNU_HASH) {
        let (nb, symoff, bloom) = (rd32(img, h)? as u64, rd32(img, h + 4)? as u64, rd32(img, h + 8)? as u64);
        let buckets = h + 16 + bloom * 8;
        let mut last = 0u64;
        for i in 0..nb {
            last = last.max(rd32(img, buckets + 4 * i)? as u64);
        }
        if last < symoff {
            symoff
        } else {
            let chains = buckets + 4 * nb;
            while rd32(img, chains + 4 * (last - symoff))? & 1 == 0 {
                last += 1;
            }
            last + 1
        }
    } else {
        0
    };
    if let Some(&st) = tags.get(&DT_SYMTAB) {
        for i in 0..nsyms {
            let e = st + 24 * i;
            d.syms.push(Sym {
                name: cstr(strtab + rd32(img, e)? as u64),
                info: img[e as usize + 4],
                shndx: u16::from_le_bytes([img[e as usize + 6], img[e as usize + 7]]),
                value: rd64(img, e + 8)?,
            });
        }
    }
    for (bt, st) in [(DT_RELA, DT_RELASZ), (DT_JMPREL, DT_PLTRELSZ)] {
        if let Some(&b) = tags.get(&bt) {
            for i in 0..tags.get(&st).copied().unwrap_or(0) / 24 {
                let e = b + 24 * i;
                let info = rd64(img, e + 8)?;
                d.relocs.push(Reloc { off: rd64(img, e)?, typ: info as u32, sym: (info >> 32) as u32, addend: rd64(img, e + 16)? as i64 });
            }
        }
    }
    if let Some(&relr) = tags.get(&DT_RELR) {
        let mut where_ = 0u64;
        for i in 0..tags.get(&DT_RELRSZ).copied().unwrap_or(0) / 8 {
            let e = rd64(img, relr + 8 * i)?;
            if e & 1 == 0 {
                d.relocs.push(Reloc { off: e, typ: R_AARCH64_RELATIVE, sym: 0, addend: rd64(img, e)? as i64 });
                where_ = e + 8;
                continue;
            }
            for k in 0..63u64 {
                if (e >> (k + 1)) & 1 != 0 {
                    let a = where_ + 8 * k;
                    d.relocs.push(Reloc { off: a, typ: R_AARCH64_RELATIVE, sym: 0, addend: rd64(img, a)? as i64 });
                }
            }
            where_ += 63 * 8;
        }
    }
    if let (Some(&a), Some(&n)) = (tags.get(&DT_INIT_ARRAY), tags.get(&DT_INIT_ARRAYSZ)) {
        d.init_array = (a, n);
    }
    Ok(d)
}

/// A module placed in guest memory.
#[derive(Debug, Clone)]
pub struct Loaded {
    pub name: String,
    pub base: u64,
    pub size: u64,
    pub image: Image,
    pub dynamic: Dynamic,
}

impl Loaded {
    pub fn entry(&self) -> u64 {
        self.base + self.image.text.0
    }
}

/// What `bind` resolved, for the load report.
#[derive(Debug, Default, Clone)]
pub struct BindReport {
    pub relocations: usize,
    pub host_bound: Vec<String>,
    pub unresolved: Vec<String>,
}

/// Places the modules (name, base, file bytes), relocates them and binds their imports. `size` checks the image
/// size the translator's layout assumed (0 to skip).
pub fn load(space: &Space, mods: &[(String, u64, u64, Vec<u8>)]) -> Result<(Vec<Loaded>, BindReport), String> {
    let mut loaded = vec![];
    for (name, base, size, raw) in mods {
        let image = parse(raw).map_err(|e| format!("{name}: {e}"))?;
        let n = image.bytes.len() as u64;
        if *size != 0 && *size != n {
            return Err(format!("{name}: the image is {n:#x} bytes; the translated code was laid out for {size:#x}"));
        }
        let dynamic = dynamic(&image.bytes).map_err(|e| format!("{name}: {e}"))?;
        space.map_module(*base, n)?;
        // SAFETY: just committed [base, base+n).
        unsafe { core::ptr::copy_nonoverlapping(image.bytes.as_ptr(), *base as *mut u8, n as usize) };
        loaded.push(Loaded { name: name.clone(), base: *base, size: n, image, dynamic });
    }
    let report = relocate(&loaded)?;
    for m in &loaded {
        let i = &m.image;
        let ro_end = i.data.0;
        space.protect_segment(m.base + i.text.0, i.text.1, ST_CODE, mem::PERM_RX)?;
        space.protect_segment(m.base + i.rodata.0, ro_end - i.rodata.0, ST_CODE, mem::PERM_R)?;
        space.protect_segment(m.base + i.data.0, m.size - i.data.0, ST_CODEDATA, mem::PERM_RW)?;
    }
    Ok((loaded, report))
}

/// Resolves symbol `name` for an import: host override, else the first module (load order) defining it.
fn resolve(mods: &[Loaded], name: &str) -> Option<u64> {
    if let Some(a) = crate::hostfn::bind(name) {
        return Some(a);
    }
    for m in mods {
        if let Some(s) = m.dynamic.syms.iter().find(|s| s.defined() && s.name == name) {
            return Some(m.base.wrapping_add(s.value));
        }
    }
    None
}

/// Applies every relocation of every module (they are mapped and writable).
pub fn relocate(mods: &[Loaded]) -> Result<BindReport, String> {
    let mut rep = BindReport::default();
    let mut cache: HashMap<String, Option<u64>> = HashMap::new();
    for m in mods {
        for r in &m.dynamic.relocs {
            let at = m.base + r.off;
            if r.off + 8 > m.size {
                return Err(format!("{}: relocation at {:#x} outside the image", m.name, r.off));
            }
            let v = match r.typ {
                R_AARCH64_NONE => continue,
                R_AARCH64_RELATIVE => m.base.wrapping_add(r.addend as u64),
                R_AARCH64_ABS64 | R_AARCH64_GLOB_DAT | R_AARCH64_JUMP_SLOT => {
                    let s = m.dynamic.syms.get(r.sym as usize).ok_or(format!("{}: bad symbol index {}", m.name, r.sym))?;
                    let target = if s.name.is_empty() {
                        Some(m.base.wrapping_add(s.value))
                    } else {
                        *cache.entry(s.name.clone()).or_insert_with(|| {
                            let a = resolve(mods, &s.name);
                            if a.is_some_and(|a| a >= crate::hostfn::HOSTFN_BASE) {
                                rep.host_bound.push(s.name.clone());
                            }
                            a
                        })
                    };
                    match target {
                        Some(t) => t.wrapping_add(if r.typ == R_AARCH64_JUMP_SLOT { 0 } else { r.addend as u64 }),
                        None if s.weak() => 0,
                        None => {
                            rep.unresolved.push(s.name.clone());
                            let a = crate::hostfn::unresolved(&s.name);
                            cache.insert(s.name.clone(), Some(a));
                            a
                        }
                    }
                }
                t => return Err(format!("{}: unsupported relocation type {t} at {:#x}", m.name, r.off)),
            };
            // SAFETY: inside the mapped, writable image.
            unsafe { core::ptr::write_unaligned(at as *mut u64, v) };
            rep.relocations += 1;
        }
    }
    rep.host_bound.sort();
    rep.host_bound.dedup();
    rep.unresolved.sort();
    rep.unresolved.dedup();
    Ok(rep)
}

/// `main.npdm`'s main-thread parameters: (priority, core, stack size). Defaults: 44, 0, 1 MiB.
pub fn npdm_main_thread(npdm: Option<&[u8]>) -> (i32, i32, u64) {
    match npdm {
        Some(b) if b.len() >= 0x20 && &b[..4] == b"META" => (b[0xE] as i32, b[0xF] as i32, rd32(b, 0x1C).unwrap_or(0x10_0000) as u64),
        _ => (44, 0, 0x10_0000),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A tiny module image built by hand: text with the MOD0 pointer, a .dynamic with symbols and relocations.
    /// Layout (offsets): 0x0 text (b +8; .word mod0), 0x1000 rodata (MOD0, dynamic, dynsym, dynstr, rela), 0x2000
    /// data (GOT: 4 words), bss 0x1000.
    pub fn tiny_image(defs: &[(&str, u64)], imports: &[(&str, u32)]) -> Vec<u8> {
        let mut img = vec![0u8; 0x3000];
        let w32 = |img: &mut Vec<u8>, o: usize, v: u32| img[o..o + 4].copy_from_slice(&v.to_le_bytes());
        let w64 = |img: &mut Vec<u8>, o: usize, v: u64| img[o..o + 8].copy_from_slice(&v.to_le_bytes());
        w32(&mut img, 0, 0x1400_0002);
        w32(&mut img, 4, 0x1000);
        // MOD0 at 0x1000: dynamic at +0x40, bss [0x2100, 0x3000), eh_frame_hdr 0, module object 0
        img[0x1000..0x1004].copy_from_slice(b"MOD0");
        for (i, v) in [0x40i32, 0x1100, 0x2000, 0, 0, 0].iter().enumerate() {
            w32(&mut img, 0x1004 + 4 * i, *v as u32);
        }
        let (dynsym, dynstr, rela, hash) = (0x1200usize, 0x1400usize, 0x1600usize, 0x1800usize);
        let mut names = vec![0u8];
        let mut syms: Vec<(u32, u8, u16, u64)> = vec![(0, 0, 0, 0)];
        for (n, v) in defs {
            syms.push((names.len() as u32, 0x12, 1, *v));
            names.extend_from_slice(n.as_bytes());
            names.push(0);
        }
        let first_import = syms.len();
        for (n, _) in imports {
            syms.push((names.len() as u32, 0x10, 0, 0));
            names.extend_from_slice(n.as_bytes());
            names.push(0);
        }
        for (i, (name, info, shndx, value)) in syms.iter().enumerate() {
            let e = dynsym + 24 * i;
            w32(&mut img, e, *name);
            img[e + 4] = *info;
            img[e + 6..e + 8].copy_from_slice(&shndx.to_le_bytes());
            w64(&mut img, e + 8, *value);
        }
        img[dynstr..dynstr + names.len()].copy_from_slice(&names);
        w32(&mut img, hash + 4, syms.len() as u32);
        // relocations: a RELATIVE at GOT[0] -> 0x10, then one per import (type given) at GOT[1+i]
        let mut rels = vec![(0x2000u64, R_AARCH64_RELATIVE as u64, 0x10i64)];
        for (i, (_, t)) in imports.iter().enumerate() {
            rels.push((0x2008 + 8 * i as u64, ((first_import + i) as u64) << 32 | *t as u64, if *t == R_AARCH64_ABS64 { 4 } else { 0 }));
        }
        for (i, (o, info, a)) in rels.iter().enumerate() {
            w64(&mut img, rela + 24 * i, *o);
            w64(&mut img, rela + 24 * i + 8, *info);
            w64(&mut img, rela + 24 * i + 16, *a as u64);
        }
        let mut d = 0x1040;
        for (t, v) in [
            (DT_HASH, hash as u64),
            (DT_STRTAB, dynstr as u64),
            (DT_SYMTAB, dynsym as u64),
            (DT_RELA, rela as u64),
            (DT_RELASZ, 24 * rels.len() as u64),
            (DT_NULL, 0),
        ] {
            w64(&mut img, d, t);
            w64(&mut img, d + 8, v);
            d += 16;
        }
        img
    }

    /// Wraps an image in an uncompressed NSO: text [0,0x1000), rodata [0x1000,0x2000), data [0x2000,0x2100), bss.
    pub fn nso(img: &[u8]) -> Vec<u8> {
        let mut f = vec![0u8; 0x100];
        f[..4].copy_from_slice(b"NSO0");
        let segs = [(0x100u32, 0u32, 0x1000u32), (0x1100, 0x1000, 0x1000), (0x2100, 0x2000, 0x100)];
        for (i, (fo, mo, sz)) in segs.iter().enumerate() {
            f[0x10 + 0x10 * i..0x14 + 0x10 * i].copy_from_slice(&fo.to_le_bytes());
            f[0x14 + 0x10 * i..0x18 + 0x10 * i].copy_from_slice(&mo.to_le_bytes());
            f[0x18 + 0x10 * i..0x1C + 0x10 * i].copy_from_slice(&sz.to_le_bytes());
            f[0x60 + 4 * i..0x64 + 4 * i].copy_from_slice(&sz.to_le_bytes());
        }
        f[0x3C..0x40].copy_from_slice(&0xF00u32.to_le_bytes());
        f.extend_from_slice(&img[..0x2100]);
        f
    }

    unsafe fn host_nop(_c: *mut crate::cpu::Cpu) {}

    #[test]
    fn parse_load_relocate_bind() {
        let main = nso(&tiny_image(&[("shared_fn", 0x40)], &[("sdk_fn", R_AARCH64_JUMP_SLOT), ("loader_test_hosted", R_AARCH64_GLOB_DAT), ("nobody_has_this", R_AARCH64_JUMP_SLOT)]));
        let sdk = nso(&tiny_image(&[("sdk_fn", 0x80), ("loader_test_hosted", 0x90)], &[("shared_fn", R_AARCH64_ABS64)]));
        let img = parse(&main).unwrap();
        assert_eq!(img.text, (0, 0x1000));
        assert_eq!(img.bytes.len(), 0x3000);
        let d = dynamic(&img.bytes).unwrap();
        assert_eq!((d.mod0, d.dynamic, d.bss_start, d.bss_end), (0x1000, 0x1040, 0x2100, 0x3000));
        assert!(d.syms.iter().any(|s| s.name == "shared_fn" && s.defined()));

        crate::hostfn::add_override("loader_test_hosted", host_nop);
        let base = 0x7B_0000_0000u64;
        let space = Space::new(mem::Range { base, size: 0x40_0000 }, 0x7B_8000_0000, false).unwrap();
        let mods = vec![("main".to_string(), base, 0x3000, main), ("sdk".to_string(), base + 0x20_0000, 0, sdk)];
        let (l, rep) = load(&space, &mods).unwrap();
        assert_eq!(l[1].entry(), base + 0x20_0000);
        let rd = |a: u64| unsafe { *(a as *const u64) };
        assert_eq!(rd(base + 0x2000), base + 0x10, "RELATIVE");
        assert_eq!(rd(base + 0x2008), base + 0x20_0000 + 0x80, "JUMP_SLOT into sdk");
        let hosted = rd(base + 0x2010);
        assert_eq!(crate::hostfn::name_of(hosted).as_deref(), Some("loader_test_hosted"), "override beats the definition");
        let missing = rd(base + 0x2018);
        assert_eq!(crate::hostfn::name_of(missing).as_deref(), Some("nobody_has_this"));
        assert_eq!(rd(base + 0x20_0000 + 0x2008), base + 0x40 + 4, "ABS64 + addend into main");
        assert_eq!(rep.unresolved, vec!["nobody_has_this".to_string()]);
        assert_eq!(rep.host_bound, vec!["loader_test_hosted".to_string()]);
        assert_eq!(unsafe { *((base + 0x2F00) as *const u8) }, 0, "bss zeroed");
        assert_eq!(space.query(base + 0x10).state, ST_CODE);
        assert_eq!(space.query(base + 0x2010).state, ST_CODEDATA);
        assert!(load(&space, &[("x".into(), base + 0x30_0000, 0x5000, nso(&tiny_image(&[], &[])))]).is_err(), "size mismatch");
    }

    #[test]
    fn nro_parse_and_npdm() {
        let img = tiny_image(&[], &[]);
        let mut nro = img[..0x2100].to_vec();
        nro[0x10..0x14].copy_from_slice(b"NRO0");
        for (i, (o, n)) in [(0u32, 0x1000u32), (0x1000, 0x1000), (0x2000, 0x100)].iter().enumerate() {
            nro[0x20 + 8 * i..0x24 + 8 * i].copy_from_slice(&o.to_le_bytes());
            nro[0x24 + 8 * i..0x28 + 8 * i].copy_from_slice(&n.to_le_bytes());
        }
        nro[0x38..0x3C].copy_from_slice(&0xF00u32.to_le_bytes());
        let i = parse(&nro).unwrap();
        assert_eq!((i.data, i.bss, i.bytes.len()), ((0x2000, 0x100), 0xF00, 0x3000));
        assert_eq!(dynamic(&i.bytes).unwrap().mod0, 0x1000);
        assert_eq!(npdm_main_thread(None), (44, 0, 0x10_0000));
        let mut meta = vec![0u8; 0x80];
        meta[..4].copy_from_slice(b"META");
        meta[0xE] = 49;
        meta[0xF] = 1;
        meta[0x1C..0x20].copy_from_slice(&0x8000u32.to_le_bytes());
        assert_eq!(npdm_main_thread(Some(&meta)), (49, 1, 0x8000));
    }
}
