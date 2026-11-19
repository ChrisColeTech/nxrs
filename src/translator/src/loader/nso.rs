//! NSO loader: decompress segments, build the memory image, parse MOD0/dynamic info.

use std::collections::HashMap;

pub const DT_NULL: u64 = 0;
pub const DT_NEEDED: u64 = 1;
pub const DT_PLTRELSZ: u64 = 2;
pub const DT_HASH: u64 = 4;
pub const DT_STRTAB: u64 = 5;
pub const DT_SYMTAB: u64 = 6;
pub const DT_RELA: u64 = 7;
pub const DT_RELASZ: u64 = 8;
pub const DT_JMPREL: u64 = 23;
pub const DT_RELRSZ: u64 = 0x23;
pub const DT_RELR: u64 = 0x24;
pub const DT_GNU_HASH: u64 = 0x6FFF_FEF5;
pub const R_AARCH64_ABS64: u32 = 257;
pub const R_AARCH64_GLOB_DAT: u32 = 1025;
pub const R_AARCH64_JUMP_SLOT: u32 = 1026;
pub const R_AARCH64_RELATIVE: u32 = 1027;

#[derive(Debug, thiserror::Error)]
pub enum NsoError {
    #[error("not an NSO (no NSO0 magic)")]
    Magic,
    #[error("segment {0}: {1}")]
    Segment(usize, String),
    #[error("no MOD0 header")]
    Mod0,
    #[error("out of bounds read at {0:#x}")]
    Bounds(usize),
}

/// A dynamic symbol.
#[derive(Debug, Clone)]
pub struct Sym {
    pub name: String,
    pub info: u8,
    pub shndx: u16,
    pub value: u64,
    pub size: u64,
}

/// A relocation: offset, type, symbol index, addend.
#[derive(Debug, Clone, Copy)]
pub struct Reloc {
    pub off: u64,
    pub typ: u32,
    pub sym: u32,
    pub addend: i64,
}

#[derive(Debug, Clone)]
pub struct Nso {
    /// Hex of the build id with trailing zero bytes stripped (Python `.rstrip(b'\0').hex()`).
    pub build_id: String,
    pub bss_size: u32,
    pub text: (usize, usize),
    pub rodata: (usize, usize),
    pub data: (usize, usize),
    pub size: usize,
    pub image: Vec<u8>,
    pub dynamic: usize,
    pub bss_start: usize,
    pub bss_end: usize,
    pub eh_start: usize,
    pub eh_end: usize,
    pub module_obj: usize,
    pub dyn_: HashMap<u64, u64>,
    pub strtab: u64,
    pub needed: Vec<String>,
    pub syms: Vec<Sym>,
    pub relocs: Vec<Reloc>,
}

fn rd32(b: &[u8], o: usize) -> Result<u32, NsoError> {
    b.get(o..o + 4).map(|s| u32::from_le_bytes(s.try_into().unwrap())).ok_or(NsoError::Bounds(o))
}

impl Nso {
    pub fn parse(raw: &[u8]) -> Result<Nso, NsoError> {
        if raw.get(..4) != Some(b"NSO0") {
            return Err(NsoError::Magic);
        }
        let flags = rd32(raw, 0xC)?;
        let bid = raw.get(0x40..0x60).ok_or(NsoError::Bounds(0x40))?;
        let end = bid.iter().rposition(|&b| b != 0).map_or(0, |p| p + 1);
        let build_id = bid[..end].iter().map(|b| format!("{b:02x}")).collect::<String>();
        let mut segs = Vec::new();
        for i in 0..3 {
            let file_off = rd32(raw, 0x10 + i * 0x10)? as usize;
            let mem_off = rd32(raw, 0x14 + i * 0x10)? as usize;
            let dec_size = rd32(raw, 0x18 + i * 0x10)? as usize;
            let comp_size = rd32(raw, 0x60 + i * 4)? as usize;
            let data = raw
                .get(file_off..file_off + comp_size)
                .ok_or_else(|| NsoError::Segment(i, "truncated".into()))?;
            let data = if flags & (1 << i) != 0 {
                lz4_flex::block::decompress(data, dec_size).map_err(|e| NsoError::Segment(i, e.to_string()))?
            } else {
                data.to_vec()
            };
            segs.push((mem_off, data));
        }
        let bss_size = rd32(raw, 0x3C)?;
        let text = (segs[0].0, segs[0].1.len());
        let rodata = (segs[1].0, segs[1].1.len());
        let data = (segs[2].0, segs[2].1.len());
        let end = segs[2].0 + segs[2].1.len() + bss_size as usize;
        let size = (end + 0xFFF) & !0xFFF;
        let mut image = vec![0u8; size];
        for (off, d) in &segs {
            image[*off..*off + d.len()].copy_from_slice(d);
        }
        let mut n = Nso {
            build_id,
            bss_size,
            text,
            rodata,
            data,
            size,
            image,
            dynamic: 0,
            bss_start: 0,
            bss_end: 0,
            eh_start: 0,
            eh_end: 0,
            module_obj: 0,
            dyn_: HashMap::new(),
            strtab: 0,
            needed: vec![],
            syms: vec![],
            relocs: vec![],
        };
        n.parse_mod0()?;
        Ok(n)
    }

    pub fn u32(&self, off: usize) -> u32 {
        u32::from_le_bytes(self.image[off..off + 4].try_into().unwrap())
    }

    pub fn u64(&self, off: usize) -> u64 {
        u64::from_le_bytes(self.image[off..off + 8].try_into().unwrap())
    }

    fn parse_mod0(&mut self) -> Result<(), NsoError> {
        let mod0 = self.u32(4) as usize;
        if self.image.get(mod0..mod0 + 4) != Some(b"MOD0") {
            return Err(NsoError::Mod0);
        }
        let rel = |s: &Self, i: usize| -> usize {
            let d = i32::from_le_bytes(s.image[mod0 + 4 + i * 4..mod0 + 8 + i * 4].try_into().unwrap());
            (mod0 as i64 + d as i64) as usize
        };
        self.dynamic = rel(self, 0);
        self.bss_start = rel(self, 1);
        self.bss_end = rel(self, 2);
        self.eh_start = rel(self, 3);
        self.eh_end = rel(self, 4);
        self.module_obj = rel(self, 5);
        let mut dyn_ = HashMap::new();
        let mut needed = vec![];
        let mut off = self.dynamic;
        loop {
            let (tag, val) = (self.u64(off), self.u64(off + 8));
            off += 16;
            if tag == DT_NULL {
                break;
            }
            if tag == DT_NEEDED {
                needed.push(val);
            }
            dyn_.entry(tag).or_insert(val);
        }
        self.dyn_ = dyn_;
        let strtab = *self.dyn_.get(&DT_STRTAB).unwrap_or(&0);
        self.strtab = strtab;
        self.needed = needed.iter().map(|&v| self.cstr((strtab + v) as usize)).collect();
        let nsyms = self.symbol_count();
        if nsyms > 0 {
            if let Some(&st) = self.dyn_.get(&DT_SYMTAB) {
                for i in 0..nsyms {
                    let o = st as usize + i * 24;
                    let name = self.u32(o);
                    let info = self.image[o + 4];
                    let shndx = u16::from_le_bytes([self.image[o + 6], self.image[o + 7]]);
                    let value = self.u64(o + 8);
                    let size = self.u64(o + 16);
                    let name = self.cstr((strtab + name as u64) as usize);
                    self.syms.push(Sym { name, info, shndx, value, size });
                }
            }
        }
        for (base_tag, size_tag) in [(DT_RELA, DT_RELASZ), (DT_JMPREL, DT_PLTRELSZ)] {
            if let Some(&b) = self.dyn_.get(&base_tag) {
                let n = self.dyn_.get(&size_tag).copied().unwrap_or(0) / 24;
                for i in 0..n as usize {
                    let o = b as usize + i * 24;
                    let r_off = self.u64(o);
                    let r_info = self.u64(o + 8);
                    let r_add = self.u64(o + 16) as i64;
                    self.relocs.push(Reloc { off: r_off, typ: r_info as u32, sym: (r_info >> 32) as u32, addend: r_add });
                }
            }
        }
        // DT_RELR: packed RELATIVE relocations whose addend is the word in place.
        if let Some(&relr) = self.dyn_.get(&DT_RELR) {
            let mut where_ = 0u64;
            let cnt = self.dyn_.get(&DT_RELRSZ).copied().unwrap_or(0) / 8;
            for i in 0..cnt as usize {
                let e = self.u64(relr as usize + i * 8);
                if e & 1 == 0 {
                    let v = self.u64(e as usize);
                    self.relocs.push(Reloc { off: e, typ: R_AARCH64_RELATIVE, sym: 0, addend: v as i64 });
                    where_ = e + 8;
                    continue;
                }
                for k in 0..63u64 {
                    if (e >> (k + 1)) & 1 != 0 {
                        let a = where_ + k * 8;
                        let v = self.u64(a as usize);
                        self.relocs.push(Reloc { off: a, typ: R_AARCH64_RELATIVE, sym: 0, addend: v as i64 });
                    }
                }
                where_ += 63 * 8;
            }
        }
        Ok(())
    }

    /// Number of .dynsym entries: DT_HASH nchain, else one past the last symbol a DT_GNU_HASH chain reaches.
    pub fn symbol_count(&self) -> usize {
        if let Some(&h) = self.dyn_.get(&DT_HASH) {
            return self.u32(h as usize + 4) as usize;
        }
        let Some(&h) = self.dyn_.get(&DT_GNU_HASH) else { return 0 };
        let h = h as usize;
        let (nbuckets, symoffset, bloom_size) = (self.u32(h) as usize, self.u32(h + 4), self.u32(h + 8) as usize);
        let buckets = h + 16 + bloom_size * 8;
        let mut last = (0..nbuckets).map(|i| self.u32(buckets + 4 * i)).max().unwrap_or(0);
        if last < symoffset {
            return symoffset as usize;
        }
        let chains = buckets + 4 * nbuckets;
        while self.u32(chains + 4 * (last - symoffset) as usize) & 1 == 0 {
            last += 1;
        }
        last as usize + 1
    }

    pub fn cstr(&self, off: usize) -> String {
        if off >= self.image.len() {
            return String::new();
        }
        let end = self.image[off..].iter().position(|&b| b == 0).map_or(self.image.len(), |p| off + p);
        String::from_utf8_lossy(&self.image[off..end]).into_owned()
    }
}
