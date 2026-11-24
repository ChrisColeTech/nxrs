//! One loaded NSO module: image at a guest base, .text as words, function seeds.
//! Lifting lives in `aot`, not here.

use crate::loader::nso::{Nso, R_AARCH64_RELATIVE};
use std::collections::BTreeSet;

pub struct Module {
    pub name: String,
    pub nso: Nso,
    pub base: u64,
    pub image: Vec<u8>,
    pub lo: u64,
    pub hi: u64,
    pub words: Vec<u32>,
    /// Function starts beside the module's own (the patch table's branch-ins, a stubs module's entries).
    pub extra_seeds: Vec<u64>,
}

fn read_encoded(img: &[u8], off: usize, enc: u8, pc_base: usize) -> Result<(i64, usize), String> {
    let (fmt, app) = (enc & 0x0F, enc & 0x70);
    let g = |n: usize| -> Result<&[u8], String> {
        img.get(off..off + n)
            .ok_or("eh_frame_hdr out of bounds".into())
    };
    let (mut v, size) = match fmt {
        0x0B => (i32::from_le_bytes(g(4)?.try_into().unwrap()) as i64, 4),
        0x03 => (u32::from_le_bytes(g(4)?.try_into().unwrap()) as i64, 4),
        0x0C => (i64::from_le_bytes(g(8)?.try_into().unwrap()), 8),
        0x04 => (u64::from_le_bytes(g(8)?.try_into().unwrap()) as i64, 8),
        _ => return Err(format!("unsupported pointer encoding {enc:#x}")),
    };
    if app == 0x10 {
        v += off as i64;
    } else if app == 0x30 {
        v += pc_base as i64;
    }
    Ok((v, size))
}

impl Module {
    pub fn new(name: &str, nso: Nso, base: u64) -> Module {
        let image = nso.image.clone();
        let lo = base + nso.text.0 as u64;
        let hi = lo + nso.text.1 as u64;
        let t0 = nso.text.0;
        let words = (0..nso.text.1 / 4)
            .map(|i| u32::from_le_bytes(image[t0 + 4 * i..t0 + 4 * i + 4].try_into().unwrap()))
            .collect();
        Module {
            name: name.to_string(),
            nso,
            base,
            image,
            lo,
            hi,
            words,
            extra_seeds: vec![],
        }
    }

    /// A module of code alone at `code` (a stubs module: `base` is its state page, zeros): no symbols, relocations
    /// or unwind tables; `entries` are its function starts.
    pub fn code(name: &str, base: u64, code: u64, words: Vec<u32>, entries: Vec<u64>) -> Module {
        let off = (code - base) as usize;
        let mut image = vec![0u8; off];
        image.extend(words.iter().flat_map(|w| w.to_le_bytes()));
        let n = image.len();
        let nso = Nso {
            build_id: String::new(),
            bss_size: 0,
            text: (off, 4 * words.len()),
            rodata: (n, 0),
            data: (n, 0),
            size: n,
            image: image.clone(),
            dynamic: 0,
            bss_start: n,
            bss_end: n,
            eh_start: 0,
            eh_end: 0,
            module_obj: 0,
            dyn_: Default::default(),
            strtab: 0,
            needed: vec![],
            syms: vec![],
            relocs: vec![],
        };
        let hi = code + 4 * words.len() as u64;
        Module {
            name: name.to_string(),
            nso,
            base,
            image,
            lo: code,
            hi,
            words,
            extra_seeds: entries,
        }
    }

    pub fn in_text(&self, a: u64) -> bool {
        self.lo <= a && a < self.hi && a % 4 == 0
    }

    pub fn in_image(&self, a: u64, n: u64) -> bool {
        self.base <= a && a + n <= self.base + self.image.len() as u64
    }

    pub fn word(&self, a: u64) -> u32 {
        self.words[((a - self.lo) >> 2) as usize]
    }

    /// Little-endian integer of n bytes at guest address a (unrelocated image). Like the Python slice, bytes past
    /// the end of the image are missing (a shorter value).
    pub fn read(&self, a: u64, n: usize, signed: bool) -> i128 {
        let o = a.wrapping_sub(self.base) as usize;
        let s = if o >= self.image.len() {
            &[][..]
        } else {
            &self.image[o..(o + n).min(self.image.len())]
        };
        let mut v: u128 = 0;
        for (i, b) in s.iter().enumerate() {
            v |= (*b as u128) << (8 * i);
        }
        if signed && !s.is_empty() && s[s.len() - 1] & 0x80 != 0 {
            v as i128 - (1i128 << (8 * s.len()))
        } else {
            v as i128
        }
    }

    /// Function starts from .eh_frame_hdr's binary search table (absolute addresses).
    pub fn eh_frame_functions(&self) -> Vec<u64> {
        let img = &self.image;
        let hdr = self.nso.eh_start;
        if hdr == 0 || hdr + 4 > img.len() {
            return vec![];
        }
        let (ptr_enc, cnt_enc, tbl_enc) = (img[hdr + 1], img[hdr + 2], img[hdr + 3]);
        let mut off = hdr + 4;
        let Ok((_, sz)) = read_encoded(img, off, ptr_enc, hdr) else {
            return vec![];
        };
        off += sz;
        let Ok((count, sz)) = read_encoded(img, off, cnt_enc, hdr) else {
            return vec![];
        };
        off += sz;
        let mut starts = vec![];
        for _ in 0..count.max(0) {
            let Ok((loc, sz)) = read_encoded(img, off, tbl_enc, hdr) else {
                break;
            };
            off += sz;
            let Ok((_, sz)) = read_encoded(img, off, tbl_enc, hdr) else {
                break;
            };
            off += sz;
            starts.push((self.base as i64 + loc) as u64);
        }
        starts
    }

    /// Known function starts: .eh_frame_hdr, exported FUNC symbols, R_AARCH64_RELATIVE targets in .text, and code
    /// reached by name: every relocation naming a symbol this module defines in .text, and every global untyped
    /// (STT_NOTYPE) symbol it defines there. Hand-written routines are exported untyped and reached only through the
    /// PLT (Mario Kart 8 Deluxe's sdk: `nndetailCryptoBignum*Words`, `nndetailCryptoBignum_{mul,sqr}_comba8`), so
    /// neither unwind entries nor FUNC symbols find them.
    pub fn seeds(&self) -> Vec<u64> {
        let mut starts: BTreeSet<u64> = self.eh_frame_functions().into_iter().collect();
        for s in &self.nso.syms {
            let (kind, bind) = (s.info & 0xF, s.info >> 4);
            if s.shndx != 0 && (kind == 2 || (kind == 0 && bind != 0 && !s.name.is_empty())) {
                starts.insert(self.base.wrapping_add(s.value));
            }
        }
        for r in &self.nso.relocs {
            if r.typ == R_AARCH64_RELATIVE {
                starts.insert(self.base.wrapping_add(r.addend as u64));
            } else if let Some(s) = self
                .nso
                .syms
                .get(r.sym as usize)
                .filter(|s| r.sym != 0 && s.shndx != 0)
            {
                if matches!(s.info & 0xF, 0 | 2) {
                    starts.insert(self.base.wrapping_add(s.value));
                }
            }
        }
        starts.extend(self.extra_seeds.iter().copied());
        starts.into_iter().filter(|&a| self.in_text(a)).collect()
    }
}
