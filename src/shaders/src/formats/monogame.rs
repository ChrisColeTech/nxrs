//! MonoGame (Celeste, Bastion, Stardew Valley): effects ('MGFX' version 8: control section + code area per shader;
//! version 5: a whole GLSLC output per shader) in XNB files and in main. Compressed XNB content is LZX (XMemCompress's,
//! a 64 KB window in MonoGame's block framing) or one raw LZ4 block.

use crate::formats::glslc;
use crate::nvn::*;

pub const MGFX: &[u8; 4] = b"MGFX";

const EXTRA: [u8; 51] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13, 14, 14, 15, 15, 16, 16, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17,
];
const BASE: [u32; 51] = [
    0, 1, 2, 3, 4, 6, 8, 12, 16, 24, 32, 48, 64, 96, 128, 192, 256, 384, 512, 768, 1024, 1536, 2048, 3072, 4096, 6144, 8192, 12288, 16384, 24576, 32768, 49152, 65536, 98304, 131072,
    196608, 262144, 393216, 524288, 655360, 786432, 917504, 1048576, 1179648, 1310720, 1441792, 1572864, 1703936, 1835008, 1966080, 2097152,
];
const MAIN_SYMS: usize = 256 + 50 * 8;
const LEN_SYMS: usize = 249 + 1;

/// A Huffman lookup table from canonical code lengths (the cabextract decoder's layout: direct entries for codes up
/// to `nbits`, a binary tree after them for longer ones).
fn table(nsyms: usize, nbits: u32, length: &[u8]) -> Result<Vec<u16>, String> {
    let mut t = vec![0u16; (1 << nbits) + nsyms * 2];
    let (mut pos, mut bit_num, mut table_mask) = (0usize, 1u32, 1usize << nbits);
    let mut bit_mask = table_mask >> 1;
    let mut next_symbol = bit_mask;
    while bit_num <= nbits {
        for sym in 0..nsyms {
            if length[sym] as u32 == bit_num {
                let leaf = pos;
                pos += bit_mask;
                if pos > table_mask {
                    return Err("LZX: decode table overrun".into());
                }
                for x in &mut t[leaf..pos] {
                    *x = sym as u16;
                }
            }
        }
        bit_mask >>= 1;
        bit_num += 1;
    }
    if pos != table_mask {
        for x in &mut t[pos..table_mask] {
            *x = 0;
        }
        pos <<= 16;
        table_mask <<= 16;
        bit_mask = 1 << 15;
        while bit_num <= 16 {
            for sym in 0..nsyms {
                if length[sym] as u32 == bit_num {
                    let mut leaf = pos >> 16;
                    for fill in 0..(bit_num - nbits) {
                        if t[leaf] == 0 {
                            if (next_symbol << 1) + 1 >= t.len() {
                                return Err("LZX: decode table overflow".into());
                            }
                            t[next_symbol << 1] = 0;
                            t[(next_symbol << 1) + 1] = 0;
                            t[leaf] = next_symbol as u16;
                            next_symbol += 1;
                        }
                        leaf = (t[leaf] as usize) << 1;
                        if (pos >> (15 - fill)) & 1 != 0 {
                            leaf += 1;
                        }
                    }
                    t[leaf] = sym as u16;
                    pos += bit_mask;
                    if pos > table_mask {
                        return Err("LZX: decode table overflow".into());
                    }
                }
            }
            bit_mask >>= 1;
            bit_num += 1;
        }
    }
    if pos != table_mask && length[..nsyms].iter().any(|&l| l != 0) {
        return Err("LZX: a bad or incomplete Huffman table".into());
    }
    Ok(t)
}

/// The LZX decoder's state, which lives for the whole XNB (window, LRU offsets, the block in progress).
pub struct Lzx {
    window: Vec<u8>,
    posn: usize,
    r: [u32; 3],
    main_elements: usize,
    header_read: bool,
    block_type: u32,
    block_remaining: usize,
    block_length: usize,
    intel_filesize: i64,
    intel_curpos: i64,
    intel_started: bool,
    frames_read: u32,
    main_len: Vec<u8>,
    length_len: Vec<u8>,
    main_tbl: Vec<u16>,
    length_tbl: Vec<u16>,
    al_tbl: Vec<u16>,
    al_len: Vec<u8>,
    // the bit reader over the current block
    ip: usize,
    bb: u64,
    bl: u32,
}

impl Lzx {
    pub fn new(window_bits: u32) -> Lzx {
        Lzx {
            window: vec![0; 1 << window_bits],
            posn: 0,
            r: [1, 1, 1],
            main_elements: 256 + ((window_bits as usize) << 1 << 3),
            header_read: false,
            block_type: 0,
            block_remaining: 0,
            block_length: 0,
            intel_filesize: 0,
            intel_curpos: 0,
            intel_started: false,
            frames_read: 0,
            main_len: vec![0; MAIN_SYMS + 64],
            length_len: vec![0; LEN_SYMS + 64],
            main_tbl: vec![],
            length_tbl: vec![],
            al_tbl: vec![],
            al_len: vec![0; 8],
            ip: 0,
            bb: 0,
            bl: 0,
        }
    }

    fn ensure(&mut self, d: &[u8], n: u32) {
        while self.bl < n {
            // a 16-bit overread past the block's end reads zeros, as the C decoder allows
            let w = if self.ip + 1 < d.len() { d[self.ip] as u64 | (d[self.ip + 1] as u64) << 8 } else { 0 };
            self.bb = (self.bb << 16) | w;
            self.bl += 16;
            self.ip += 2;
        }
    }
    fn remove(&mut self, n: u32) {
        self.bl -= n;
        self.bb &= (1u64 << self.bl) - 1;
    }
    fn read(&mut self, d: &[u8], n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        self.ensure(d, n);
        let v = (self.bb >> (self.bl - n)) as u32;
        self.remove(n);
        v
    }
    fn huffsym(&mut self, d: &[u8], which: u8, nbits: u32, maxsyms: usize) -> Result<usize, String> {
        self.ensure(d, 16);
        let (tbl, lens): (&[u16], &[u8]) = match which {
            0 => (&self.main_tbl, &self.main_len),
            1 => (&self.length_tbl, &self.length_len),
            _ => (&self.al_tbl, &self.al_len),
        };
        let mut i = *tbl.get((self.bb >> (self.bl - nbits)) as usize).ok_or("LZX: no table")? as usize;
        if i >= maxsyms {
            let mut k = 0;
            loop {
                if (self.bl as i64) - nbits as i64 - 1 - k < 0 {
                    return Err("LZX: a Huffman code longer than the bitstream".into());
                }
                i = (i << 1) | ((self.bb >> (self.bl - nbits - 1 - k as u32)) & 1) as usize;
                k += 1;
                i = *tbl.get(i).ok_or("LZX: bad Huffman code")? as usize;
                if i < maxsyms {
                    break;
                }
            }
        }
        let l = lens[i] as u32;
        self.remove(l);
        Ok(i)
    }
    fn pre_sym(&mut self, d: &[u8], pre: &[u16], pre_len: &[u8]) -> Result<usize, String> {
        self.ensure(d, 16);
        let mut i = pre[(self.bb >> (self.bl - 6)) as usize] as usize;
        if i >= 20 {
            let mut k = 0u32;
            loop {
                if self.bl < 6 + 1 + k {
                    return Err("LZX: a Huffman code longer than the bitstream".into());
                }
                i = (i << 1) | ((self.bb >> (self.bl - 6 - 1 - k)) & 1) as usize;
                k += 1;
                i = *pre.get(i).ok_or("LZX: bad pretree code")? as usize;
                if i < 20 {
                    break;
                }
            }
        }
        self.remove(pre_len[i] as u32);
        Ok(i)
    }
    fn read_lens(&mut self, d: &[u8], main: bool, first: usize, last: usize) -> Result<(), String> {
        let pre_len: Vec<u8> = (0..20).map(|_| self.read(d, 4) as u8).collect();
        let pre = table(20, 6, &pre_len)?;
        let mut x = first;
        while x < last {
            let z = self.pre_sym(d, &pre, &pre_len)?;
            fn lens(s: &mut Lzx, main: bool) -> &mut Vec<u8> { if main { &mut s.main_len } else { &mut s.length_len } }
            match z {
                17 => {
                    let y = self.read(d, 4) + 4;
                    for _ in 0..y {
                        *lens(self, main).get_mut(x).ok_or("LZX: lengths overrun")? = 0;
                        x += 1;
                    }
                }
                18 => {
                    let y = self.read(d, 5) + 20;
                    for _ in 0..y {
                        *lens(self, main).get_mut(x).ok_or("LZX: lengths overrun")? = 0;
                        x += 1;
                    }
                }
                19 => {
                    let y = self.read(d, 1) + 4;
                    let z = self.pre_sym(d, &pre, &pre_len)? as i32;
                    let cur = *lens(self, main).get(x).ok_or("LZX: lengths overrun")? as i32;
                    let mut z = cur - z;
                    if z < 0 {
                        z += 17;
                    }
                    for _ in 0..y {
                        *lens(self, main).get_mut(x).ok_or("LZX: lengths overrun")? = z as u8;
                        x += 1;
                    }
                }
                _ => {
                    let cur = lens(self, main)[x] as i32;
                    let mut z = cur - z as i32;
                    if z < 0 {
                        z += 17;
                    }
                    lens(self, main)[x] = z as u8;
                    x += 1;
                }
            }
        }
        Ok(())
    }

    /// One frame: `src` (a block of the XNB framing) → `outlen` bytes.
    pub fn decompress(&mut self, d: &[u8], outlen: usize) -> Result<Vec<u8>, String> {
        self.ip = 0;
        self.bb = 0;
        self.bl = 0;
        let mut togo = outlen;
        if !self.header_read {
            if self.read(d, 1) != 0 {
                let hi = self.read(d, 16) as i64;
                let lo = self.read(d, 16) as i64;
                self.intel_filesize = (hi << 16) | lo;
            }
            self.header_read = true;
        }
        let ws = self.window.len();
        while togo > 0 {
            if self.block_remaining == 0 {
                if self.block_type == 3 {
                    if self.block_length & 1 != 0 {
                        self.ip += 1;
                    }
                    self.bb = 0;
                    self.bl = 0;
                }
                self.block_type = self.read(d, 3);
                let hi = self.read(d, 16) as usize;
                let lo = self.read(d, 8) as usize;
                self.block_length = (hi << 8) | lo;
                self.block_remaining = self.block_length;
                if self.block_type == 2 {
                    self.al_len = (0..8).map(|_| self.read(d, 3) as u8).collect();
                    self.al_tbl = table(8, 7, &self.al_len)?;
                }
                match self.block_type {
                    1 | 2 => {
                        self.read_lens(d, true, 0, 256)?;
                        let me = self.main_elements;
                        self.read_lens(d, true, 256, me)?;
                        self.main_tbl = table(MAIN_SYMS, 12, &self.main_len)?;
                        if self.main_len[0xe8] != 0 {
                            self.intel_started = true;
                        }
                        self.read_lens(d, false, 0, 249)?;
                        self.length_tbl = table(LEN_SYMS, 12, &self.length_len)?;
                    }
                    3 => {
                        self.intel_started = true;
                        self.ensure(d, 16);
                        if self.bl > 16 {
                            self.ip -= 2;
                        }
                        for k in 0..3 {
                            self.r[k] = u32_at(d, self.ip).ok_or("LZX: uncompressed block header cut short")?;
                            self.ip += 4;
                        }
                    }
                    t => return Err(format!("LZX: block type {t}")),
                }
            }
            if self.ip > d.len() && (self.ip > d.len() + 2 || self.bl < 16) {
                return Err("LZX: the block ended early".into());
            }
            let run = self.block_remaining.min(togo);
            togo -= run;
            self.block_remaining -= run;
            self.posn &= ws - 1;
            if self.posn + run > ws {
                return Err("LZX: a run straddles the window".into());
            }
            let mut posn = self.posn;
            if self.block_type == 3 {
                let src = d.get(self.ip..self.ip + run).ok_or("LZX: the uncompressed block runs past the block")?;
                self.window[posn..posn + run].copy_from_slice(src);
                self.ip += run;
                posn += run;
            } else {
                let mut this_run = run as i64;
                while this_run > 0 {
                    let mut me = self.huffsym(d, 0, 12, MAIN_SYMS)?;
                    if me < 256 {
                        self.window[posn] = me as u8;
                        posn += 1;
                        this_run -= 1;
                        continue;
                    }
                    me -= 256;
                    let mut ml = me & 7;
                    if ml == 7 {
                        ml += self.huffsym(d, 1, 12, LEN_SYMS)?;
                    }
                    ml += 2;
                    let mut mo = (me >> 3) as u32;
                    if mo > 2 {
                        if mo as usize >= EXTRA.len() {
                            return Err("LZX: position slot out of range".into());
                        }
                        if self.block_type == 2 {
                            let mut extra = EXTRA[mo as usize] as u32;
                            let base = BASE[mo as usize] - 2;
                            mo = base;
                            if extra > 3 {
                                extra -= 3;
                                let v = self.read(d, extra);
                                mo += (v << 3) + self.huffsym(d, 2, 7, 8)? as u32;
                            } else if extra == 3 {
                                mo += self.huffsym(d, 2, 7, 8)? as u32;
                            } else if extra > 0 {
                                mo += self.read(d, extra);
                            } else {
                                mo = 1;
                            }
                        } else if mo != 3 {
                            let e = EXTRA[mo as usize] as u32;
                            mo = BASE[mo as usize] - 2 + self.read(d, e);
                        } else {
                            mo = 1;
                        }
                        self.r = [mo, self.r[0], self.r[1]];
                    } else if mo == 0 {
                        mo = self.r[0];
                    } else if mo == 1 {
                        mo = self.r[1];
                        self.r[1] = self.r[0];
                        self.r[0] = mo;
                    } else {
                        mo = self.r[2];
                        self.r[2] = self.r[0];
                        self.r[0] = mo;
                    }
                    posn += ml;
                    if posn > ws {
                        return Err("LZX: a match overruns the window".into());
                    }
                    this_run -= ml as i64;
                    let mut src = posn as i64 - ml as i64 - mo as i64;
                    if src < 0 {
                        src += ws as i64;
                    }
                    let (src, dst) = (src as usize, posn - ml);
                    for i in 0..ml {
                        self.window[dst + i] = self.window[(src + i) % ws];
                    }
                }
            }
            self.posn = posn;
        }
        let (start, end) = if self.posn != 0 { (self.posn as i64 - outlen as i64, self.posn) } else { (ws as i64 - outlen as i64, ws) };
        if start < 0 {
            return Err("LZX: frame larger than the window".into());
        }
        let mut out = self.window[start as usize..end].to_vec();
        self.frames_read += 1;
        if self.frames_read <= 32768 && self.intel_filesize != 0 {
            if outlen <= 6 || !self.intel_started {
                self.intel_curpos += outlen as i64;
            } else {
                let (mut curpos, filesize) = (self.intel_curpos, self.intel_filesize);
                self.intel_curpos = curpos + outlen as i64;
                let (mut pos, end) = (0usize, outlen.saturating_sub(10));
                while pos < end {
                    let b = out[pos];
                    pos += 1;
                    if b != 0xe8 {
                        curpos += 1;
                        continue;
                    }
                    let abs = i32::from_le_bytes(out[pos..pos + 4].try_into().unwrap()) as i64;
                    if -curpos <= abs && abs < filesize {
                        let rel = if abs >= 0 { abs - curpos } else { abs + filesize };
                        out[pos..pos + 4].copy_from_slice(&(rel as u32).to_le_bytes());
                    }
                    pos += 4;
                    curpos += 5;
                }
            }
        }
        Ok(out)
    }
}

/// Whether `d` is a compressed XNB (flags 0x80 LZX, 0x40 LZ4).
pub fn is_compressed_xnb(d: &[u8]) -> bool {
    d.len() > 14 && d.starts_with(b"XNB") && d[4] == 5 && d[5] & 0xc0 != 0
}

/// A compressed XNB's content (the u32 at 6 the file size, at 10 the content size). `first_block`: only the first LZX
/// frame (the reader names are at the content's start).
pub fn xnb_decompress(d: &[u8], first_block: bool, label: &str) -> R<Vec<u8>> {
    let dec = u32_at(d, 10).ok_or_else(|| format!("{label}: XNB header cut short"))? as usize;
    let fsize = (u32_at(d, 6).unwrap_or(0) as usize).min(d.len());
    let body = d.get(14..fsize.max(14)).unwrap_or(&[]);
    if d[5] & 0x80 != 0 {
        let mut st = Lzx::new(16);
        let mut out = Vec::with_capacity(dec);
        let mut p = 0;
        while out.len() < dec && p < body.len() {
            let hi = body[p];
            p += 1;
            let (dst, src) = if hi == 0xff {
                let b = body.get(p..p + 4).ok_or_else(|| format!("{label}: XNB LZX frame header cut short"))?;
                p += 4;
                (((b[0] as usize) << 8) | b[1] as usize, ((b[2] as usize) << 8) | b[3] as usize)
            } else {
                let b = *body.get(p).ok_or_else(|| format!("{label}: XNB LZX frame header cut short"))?;
                p += 1;
                (0x8000, ((hi as usize) << 8) | b as usize)
            };
            if src == 0 || dst == 0 {
                break;
            }
            let blk = body.get(p..p + src).ok_or_else(|| format!("{label}: XNB LZX frame runs past the file"))?;
            out.extend(st.decompress(blk, dst).map_err(|e| format!("{label}: {e}"))?);
            p += src;
            if first_block {
                break;
            }
        }
        return Ok(out);
    }
    if d[5] & 0x40 != 0 {
        return lz4_flex::block::decompress(body, dec).map_err(|e| format!("{label}: XNB LZ4 block: {e}"));
    }
    Ok(d[14..].to_vec())
}

/// A little-endian reader with .NET's BinaryReader strings.
struct Rd<'a> {
    d: &'a [u8],
    p: usize,
}

impl Rd<'_> {
    fn u8(&mut self) -> Option<u8> {
        let v = *self.d.get(self.p)?;
        self.p += 1;
        Some(v)
    }
    fn raw(&mut self, n: usize) -> Option<&[u8]> {
        let v = self.d.get(self.p..self.p + n)?;
        self.p += n;
        Some(v)
    }
    fn u32(&mut self) -> Option<u32> {
        let v = u32_at(self.d, self.p)?;
        self.p += 4;
        Some(v)
    }
    fn str(&mut self) -> Option<()> {
        let (mut n, mut sh) = (0usize, 0);
        loop {
            let b = self.u8()?;
            n |= ((b & 0x7f) as usize) << sh;
            sh += 7;
            if b < 0x80 || sh > 28 {
                break;
            }
        }
        self.raw(n).map(|_| ())
    }
}

/// A MonoGame effect at `d[p..]`. → None when the header is not version 8's or 5's or its first shader is not an NVN
/// one (four bytes 'MGFX' anywhere in other data).
pub fn effect(d: &[u8], p: usize, label: &str) -> Option<R<Vec<Found>>> {
    let v5 = d.get(p + 4) == Some(&5);
    if d.get(p + 4) != Some(&8) && !v5 {
        return None;
    }
    let mut r = Rd { d, p: p + if v5 { 6 } else { 10 } };
    let head = (|| -> Option<bool> {
        for _ in 0..r.u8()? {
            if !v5 {
                r.str()?;
            }
            r.raw(2)?;
            let n = r.u8()? as usize;
            r.raw(3 * n)?;
        }
        Some(if v5 { d.get(r.p + 6..r.p + 10) == Some(&glslc::MAGIC[..]) } else { d.get(r.p + 10..r.p + 14) == Some(&CTL_MAGIC[..]) })
    })();
    if head != Some(true) {
        return None;
    }
    let at = format!("{label} MGFX @{p:#x}");
    let xnb = label.ends_with(".xnb");
    let cut = || format!("{at}: the effect runs past the data");
    let res = (|| -> R<Option<Vec<Found>>> {
        let mut out = vec![];
        let n_sh = r.u8().ok_or_else(cut)?;
        for i in 0..n_sh {
            let vs = r.u8().ok_or_else(cut)?;
            let n = r.u32().ok_or_else(cut)? as usize;
            if r.p + n > d.len() {
                if xnb {
                    return Err(format!("{at}: shader {i} claims {n:#x} bytes past the end of the file"));
                }
                return Ok(None);
            }
            let bc = r.raw(n).unwrap();
            let (st, ctl, code): (Option<Stage>, Vec<u8>, Vec<u8>) = if v5 {
                let g = glslc::shaders(bc, &format!("{at} shader {i}"));
                if vs > 1 || g.len() != 1 || bc.get(..4) != Some(&glslc::MAGIC[..]) || u32_at(bc, 0x44) != Some(n as u32) {
                    return Err(format!("{at} shader {i}: {n:#x} bytes, not one GLSLC output of one stage"));
                }
                (Some(g[0].stage), g[0].ctl.clone(), g[0].code.clone())
            } else {
                let nctl = u32_at(bc, 0).unwrap_or(u32::MAX) as usize;
                let ncode = if 8 + nctl <= n { u32_at(bc, 4 + nctl).map(|x| x as usize) } else { None };
                if vs > 1 || ncode.map(|k| 8 + nctl + k) != Some(n) {
                    return Err(format!("{at} shader {i}: {n:#x} bytes, not a control section and a code area"));
                }
                let code = bc[8 + nctl..].to_vec();
                let st = if code.get(..4) == Some(&GFX_MAGIC[..]) { sph_stage(&code) } else { None };
                (st, bc[4..4 + nctl].to_vec(), code)
            };
            let want = if vs == 1 { Stage::Vs } else { Stage::Ps };
            let why = if st == Some(want) { check(want, &ctl, &code).map(|s| s.to_string()) } else { Some(format!("the SPH stage is {st:?}, the effect says {want:?}")) };
            if let Some(w) = why {
                return Err(format!("{at} shader {i}: {w}"));
            }
            out.push(Found::new(format!("{label}#MGFX@{p:#x}#{i}"), want, &ctl, &code));
            for _ in 0..r.u8().ok_or_else(cut)? {
                r.raw(3).ok_or_else(cut)?;
                if r.u8().ok_or_else(cut)? != 0 {
                    if v5 {
                        return Err(format!("{at} shader {i}: a sampler state (none in the version 5 effects seen)"));
                    }
                    r.raw(20).ok_or_else(cut)?;
                }
                if !v5 {
                    r.str().ok_or_else(cut)?;
                }
                r.u8().ok_or_else(cut)?;
            }
            let n = r.u8().ok_or_else(cut)? as usize;
            r.raw(n).ok_or_else(cut)?;
            if !v5 {
                for _ in 0..r.u8().ok_or_else(cut)? {
                    r.str().ok_or_else(cut)?;
                    r.raw(4).ok_or_else(cut)?;
                }
            }
        }
        // the parameters, techniques and passes, read to the end so a misread shows
        fn params(r: &mut Rd, at: &str, depth: usize) -> R<usize> {
            let cut = || format!("{at}: the parameters run past the data");
            if depth > 32 {
                return Err(format!("{at}: parameters nested too deep"));
            }
            let n = r.u8().ok_or_else(cut)? as usize;
            for _ in 0..n {
                let typ = r.raw(2).ok_or_else(cut)?[1];
                r.str().ok_or_else(cut)?;
                r.str().ok_or_else(cut)?;
                if r.u8().ok_or_else(cut)? != 0 {
                    return Err(format!("{at}: annotations (MonoGame writes none)"));
                }
                let (rows, cols) = (r.u8().ok_or_else(cut)? as usize, r.u8().ok_or_else(cut)? as usize);
                let a = params(r, at, depth + 1)?;
                let b = params(r, at, depth + 1)?;
                if a + b != 0 {
                    continue;
                }
                match typ {
                    1..=3 => {
                        r.raw(4 * rows * cols).ok_or_else(cut)?;
                    }
                    4 => r.str().ok_or_else(cut)?,
                    _ => {}
                }
            }
            Ok(n)
        }
        params(&mut r, &at, 0)?;
        for _ in 0..r.u8().ok_or_else(cut)? {
            r.str().ok_or_else(cut)?;
            if r.u8().ok_or_else(cut)? != 0 {
                return Err(format!("{at}: annotations (MonoGame writes none)"));
            }
            for _ in 0..r.u8().ok_or_else(cut)? {
                r.str().ok_or_else(cut)?;
                if r.u8().ok_or_else(cut)? != 0 {
                    return Err(format!("{at}: annotations (MonoGame writes none)"));
                }
                let (v, f) = (r.u8().ok_or_else(cut)? as usize, r.u8().ok_or_else(cut)? as usize);
                if (v != 255 && v >= out.len()) || (f != 255 && f >= out.len()) {
                    return Err(format!("{at}: a pass names shader {v} / {f} of {}", out.len()));
                }
                for n in [18usize, 25, 12] {
                    if r.u8().ok_or_else(cut)? != 0 {
                        r.raw(n).ok_or_else(cut)?;
                    }
                }
            }
        }
        if xnb && d[r.p..].iter().any(|&b| b != 0) {
            return Err(format!("{at}: the effect ends at {:#x}, the file at {:#x}", r.p, d.len()));
        }
        Ok(Some(out))
    })();
    match res {
        Ok(Some(v)) => Some(Ok(v)),
        Ok(None) => None,
        Err(e) => Some(Err(e)),
    }
}

/// Every MonoGame effect in `d`.
pub fn shaders(d: &[u8], label: &str) -> R<Vec<Found>> {
    let mut out = vec![];
    for p in memchr::memmem::find_iter(d, MGFX) {
        if let Some(r) = effect(d, p, label) {
            out.extend(r?);
        }
    }
    if label.ends_with(".xnb") && memchr::memmem::find(&d[..d.len().min(0x200)], b"EffectReader").is_some() && out.is_empty() {
        return Err(format!("{label}: an XNB effect without an MGFX version 5 or 8 effect of NVN shaders"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nvn::synth::*;

    fn effect8(vs: (Vec<u8>, Vec<u8>), ps: (Vec<u8>, Vec<u8>)) -> Vec<u8> {
        let mut e = b"MGFX".to_vec();
        e.extend([8, 0]);
        e.extend([0u8; 4]); // effect key
        e.push(0); // constant buffers
        e.push(2);
        for (st, (c, k)) in [(1u8, vs), (0u8, ps)] {
            e.push(st);
            let n = 8 + c.len() + k.len();
            e.extend((n as u32).to_le_bytes());
            e.extend((c.len() as u32).to_le_bytes());
            e.extend(&c);
            e.extend((k.len() as u32).to_le_bytes());
            e.extend(&k);
            e.push(0); // samplers
            e.push(0); // constant buffer indices
            e.push(0); // attributes
        }
        e.push(0); // parameters
        e.push(1); // technique
        e.push(4);
        e.extend(b"Main");
        e.push(0);
        e.push(1); // pass
        e.push(2);
        e.extend(b"P0");
        e.push(0);
        e.extend([0, 1, 0, 0, 0]);
        e
    }

    #[test]
    fn mgfx_v8() {
        let e = effect8(nvn1_vs(1), nvn1_ps(2));
        let f = shaders(&e, "/Content/Effects/x.xnb").unwrap();
        assert_eq!(f.iter().map(|x| x.stage).collect::<Vec<_>>(), vec![Stage::Vs, Stage::Ps]);
        // garbage after 'MGFX' in other data is no effect
        assert!(shaders(b"xxMGFX\x08\0\0\0\0\0\x00\x09", "/main").unwrap().is_empty());
    }

    #[test]
    fn lzx_uncompressed_block_and_lz4() {
        // LZX: header bit 0 (no E8), block type 3, length 8, then the aligned R0..R2 and 8 literal bytes
        // bits: 0 | 011 | 0000000000000000 | 00001000 -> 28 bits, padded to 32
        let bits: u32 = (0b0 << 27) | (0b011 << 24) | (0 << 8) | 8;
        let v = bits << 4;
        let w1 = (v >> 16) as u16;
        let w2 = v as u16;
        let mut blk = vec![];
        blk.extend(w1.to_le_bytes());
        blk.extend(w2.to_le_bytes());
        blk.extend(1u32.to_le_bytes());
        blk.extend(1u32.to_le_bytes());
        blk.extend(1u32.to_le_bytes());
        blk.extend(b"ABCDEFGH");
        let mut z = Lzx::new(16);
        assert_eq!(z.decompress(&blk, 8).unwrap(), b"ABCDEFGH");
        // an LZ4 XNB
        let content = b"EffectReader content".repeat(4);
        let c = lz4_flex::block::compress(&content);
        let mut x = b"XNBw\x05\x40".to_vec();
        x.extend(((14 + c.len()) as u32).to_le_bytes());
        x.extend((content.len() as u32).to_le_bytes());
        x.extend(&c);
        assert!(is_compressed_xnb(&x));
        assert_eq!(xnb_decompress(&x, false, "t").unwrap(), content);
    }
}
