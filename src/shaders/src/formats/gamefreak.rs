//! Game Freak's containers. GFPAK (Pokémon Sword, Legends: Arceus): a table of entries, stored, zlib, LZ4 or Oodle
//! Kraken. Trinity (Pokémon Scarlet / Violet, Legends: Z-A): `arc/data.trpfs`, FlatBuffers packs of files, Oodle.

use super::archive;
use crate::nvn::*;

pub const GFPAK: &[u8; 8] = b"GFLXPACK";
pub const ONEPACK: &[u8; 8] = b"ONEPACK\0";

/// GFPAK: 'GFLXPACK', u32 version 0x1000, u32 0, u32 file count, u32 folder count, u64 file table, ...; 0x18 bytes a
/// file {u16 level, u16 compression, u32 length, u32 stored length, u32 0, u64 data offset}: compression 0 stored,
/// 1 zlib, 2 LZ4 (one block), 3 Oodle Kraken. Calls `each(index, bytes)` per entry.
pub fn gfpak_members(d: &[u8], label: &str, each: &mut dyn FnMut(usize, Vec<u8>)) -> R<()> {
    if d.get(8..12) != Some(&[0, 0x10, 0, 0]) {
        return Err(format!("{label}: GFPAK version {:02x?}, not 0x1000", d.get(8..12)));
    }
    let n = need32(d, 0x10, label)? as usize;
    let files = need64(d, 0x18, label)? as usize;
    for i in 0..n {
        let e = files + 0x18 * i;
        let ct = need16(d, e + 2, label)?;
        let usz = need32(d, e + 4, label)? as usize;
        let csz = need32(d, e + 8, label)? as u64;
        let off = need64(d, e + 0x10, label)?;
        let raw = span(d, off, csz, &format!("{label}#{i}"))?;
        let w = format!("{label}#{i}");
        let data = match ct {
            0 => raw.to_vec(),
            1 => archive::unzlib(raw, &w)?,
            2 => lz4_flex::block::decompress(raw, usz).map_err(|e| format!("{w}: LZ4: {e}"))?,
            3 => archive::oodle(raw, usz, &w)?,
            _ => return Err(format!("{label}: entry {i} has compression {ct}, which this reader does not decode")),
        };
        if data.len() != usz {
            return Err(format!("{w}: {:#x} bytes, the entry says {usz:#x}", data.len()));
        }
        each(i, data);
    }
    Ok(())
}

/// An unsigned 32-bit FlatBuffers offset from `at` (wrapping, so a window under 4 GiB reads the same either way).
fn rel(at: usize, off: u32) -> usize {
    (at as u32).wrapping_add(off) as usize
}

/// FlatBuffers over a byte window: offsets are relative to the window.
pub struct Fb<'a>(pub &'a [u8]);

impl<'a> Fb<'a> {
    fn bad(&self, what: &str, at: usize) -> String {
        format!("FlatBuffers {what} at {at:#x} lies outside the {:#x} bytes there are", self.0.len())
    }
    pub fn root(&self, at: usize) -> R<usize> {
        Ok(rel(at, need32(self.0, at, "FlatBuffers root")?))
    }
    /// Absolute offset of field `i` of the table at `t`; None when absent.
    pub fn field(&self, t: usize, i: usize) -> R<Option<usize>> {
        let soff = i32_at(self.0, t).ok_or_else(|| self.bad("table", t))? as i64;
        let vt = t as i64 - soff;
        if vt < 0 {
            return Err(self.bad("vtable", t));
        }
        let vt = vt as usize;
        let vlen = u16_at(self.0, vt).ok_or_else(|| self.bad("vtable", vt))? as usize;
        if 4 + 2 * i >= vlen {
            return Ok(None);
        }
        let off = u16_at(self.0, vt + 4 + 2 * i).ok_or_else(|| self.bad("vtable", vt))? as usize;
        Ok(if off != 0 { Some(t + off) } else { None })
    }
    /// (start, count) of the vector in field `i`.
    pub fn vec(&self, t: usize, i: usize) -> R<(usize, usize)> {
        let Some(p) = self.field(t, i)? else { return Ok((0, 0)) };
        let p = rel(p, need32(self.0, p, "FlatBuffers vector")?);
        let n = need32(self.0, p, "FlatBuffers vector")? as usize;
        Ok((p + 4, n))
    }
    /// The tables / strings a vector of offsets points at.
    pub fn refs(&self, t: usize, i: usize) -> R<Vec<usize>> {
        let (st, n) = self.vec(t, i)?;
        (0..n).map(|k| Ok(rel(st + 4 * k, need32(self.0, st + 4 * k, "FlatBuffers offset")?))).collect()
    }
    pub fn u64s(&self, st: usize, n: usize) -> R<Vec<u64>> {
        (0..n).map(|k| need64(self.0, st + 8 * k, "FlatBuffers u64")).collect()
    }
}

/// The packs of a Trinity file system: 'ONEPACK\0', u64 offset of the root table {0 pack name hashes [u64], 1 pack
/// offsets [u64]}. `tail` is the file from `root_off` to its end. → the pack offsets, sorted.
pub fn trpfs_packs(head: &[u8], tail: &[u8]) -> R<Vec<u64>> {
    if head.get(..8) != Some(&ONEPACK[..]) {
        return Err("not a Trinity file system".into());
    }
    let fb = Fb(tail);
    let root = fb.root(0)?;
    let (st, n) = fb.vec(root, 1)?;
    let mut offs = fb.u64s(st, n)?;
    offs.sort();
    Ok(offs)
}

/// The files of one Trinity pack (its FlatBuffer from its first byte): {0 file hashes [u64], 1 files [{1 compression
/// i8 (3 Oodle), 2 level, 3 length u64, 4 data [u8]}]}. Calls `each(hash, bytes)` per file.
pub fn trinity_pack(pack: &[u8], label: &str, each: &mut dyn FnMut(u64, Vec<u8>) -> R<()>) -> R<()> {
    let fb = Fb(pack);
    let root = fb.root(0)?;
    let (hs, nh) = fb.vec(root, 0)?;
    let hashes = fb.u64s(hs, nh)?;
    for (k, t) in fb.refs(root, 1)?.into_iter().enumerate() {
        let comp = match fb.field(t, 1)? {
            Some(c) => *pack.get(c).ok_or_else(|| fb.bad("field", c))? as i8,
            None => 0,
        };
        let n = match fb.field(t, 3)? {
            Some(l) => need64(pack, l, "Trinity file length")?,
            None => 0,
        };
        let (st, sz) = fb.vec(t, 4)?;
        let raw = span(pack, st as u64, sz as u64, &format!("{label} file {k}"))?;
        let hash = hashes.get(k).copied().unwrap_or(k as u64);
        let w = format!("{label}#{hash:016x}");
        let data = if comp == 3 {
            archive::oodle(raw, n as usize, &w)?
        } else if sz as u64 == n {
            raw.to_vec()
        } else {
            return Err(format!("{w}: compression {comp} ({sz} -> {n} bytes), which is not known"));
        };
        each(hash, data)?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A minimal FlatBuffer writer for the tests: tables with fields of offsets, scalars and vectors.
    #[derive(Default)]
    pub struct FbW {
        pub b: Vec<u8>,
    }
    impl FbW {
        pub fn align(&mut self, a: usize) {
            while self.b.len() % a != 0 {
                self.b.push(0);
            }
        }
        /// A vector of bytes → its position.
        pub fn bytes(&mut self, v: &[u8]) -> usize {
            self.align(4);
            let p = self.b.len();
            self.b.extend_from_slice(&(v.len() as u32).to_le_bytes());
            self.b.extend_from_slice(v);
            p
        }
        pub fn u64s(&mut self, v: &[u64]) -> usize {
            self.align(8);
            if (self.b.len() + 4) % 8 != 0 {
                self.b.extend_from_slice(&[0; 4]);
            }
            let p = self.b.len();
            self.b.extend_from_slice(&(v.len() as u32).to_le_bytes());
            for x in v {
                self.b.extend_from_slice(&x.to_le_bytes());
            }
            p
        }
        /// A vector of offsets to tables at the given positions → its position.
        pub fn offs(&mut self, v: &[usize]) -> usize {
            self.align(4);
            let p = self.b.len();
            self.b.extend_from_slice(&(v.len() as u32).to_le_bytes());
            for (k, &t) in v.iter().enumerate() {
                let at = p + 4 + 4 * k;
                self.b.extend_from_slice(&((t as i64 - at as i64) as u32).to_le_bytes());
            }
            p
        }
        /// A table whose fields are (index, kind): Ref(pos) = an offset to a vector or table written before it (the
        /// 32-bit offset wraps backward), Raw(bytes) = an inline scalar.
        pub fn table(&mut self, fields: &[(usize, Field)]) -> usize {
            let nf = fields.iter().map(|f| f.0 + 1).max().unwrap_or(0);
            self.align(4);
            let vt = self.b.len();
            let vlen = 4 + 2 * nf;
            self.b.resize(vt + ((vlen + 3) & !3), 0);
            let t = self.b.len();
            self.b.extend_from_slice(&((t - vt) as i32).to_le_bytes());
            let mut offs = vec![0u16; nf];
            for (i, f) in fields {
                match f {
                    Field::Raw(r) => {
                        while (self.b.len() - t) % r.len().min(8) != 0 {
                            self.b.push(0);
                        }
                        offs[*i] = (self.b.len() - t) as u16;
                        self.b.extend_from_slice(r);
                    }
                    Field::Ref(_) => {
                        offs[*i] = (self.b.len() - t) as u16;
                        self.b.extend_from_slice(&[0; 4]);
                    }
                }
            }
            for (i, f) in fields {
                if let Field::Ref(p) = f {
                    let at = t + offs[*i] as usize;
                    let rel = (*p as i64 - at as i64) as u32;
                    self.b[at..at + 4].copy_from_slice(&rel.to_le_bytes());
                }
            }
            self.b[vt..vt + 2].copy_from_slice(&(vlen as u16).to_le_bytes());
            let tlen = (self.b.len() - t) as u16;
            self.b[vt + 2..vt + 4].copy_from_slice(&tlen.to_le_bytes());
            for (i, o) in offs.iter().enumerate() {
                self.b[vt + 4 + 2 * i..vt + 6 + 2 * i].copy_from_slice(&o.to_le_bytes());
            }
            t
        }
        pub fn root(&mut self, t: usize) -> Vec<u8> {
            let mut out = vec![0u8; 8];
            out[..4].copy_from_slice(&((t + 8) as u32).to_le_bytes());
            out.extend_from_slice(&self.b);
            out
        }
    }
    pub enum Field {
        Raw(Vec<u8>),
        Ref(usize),
    }

    /// A Trinity pack of the given (hash, bytes) files, stored.
    pub fn make_pack(files: &[(u64, &[u8])]) -> Vec<u8> {
        let mut w = FbW::default();
        let mut tabs = vec![];
        for (_, data) in files {
            let v = w.bytes(data);
            tabs.push(w.table(&[(1, Field::Raw(vec![0])), (3, Field::Raw((data.len() as u64).to_le_bytes().to_vec())), (4, Field::Ref(v))]));
        }
        let hs = w.u64s(&files.iter().map(|f| f.0).collect::<Vec<_>>());
        let fv = w.offs(&tabs);
        let root = w.table(&[(0, Field::Ref(hs)), (1, Field::Ref(fv))]);
        w.root(root)
    }

    #[test]
    fn trinity_pack_files() {
        let pack = make_pack(&[(0x1111, b"first file"), (0x2222, b"BNSH second")]);
        let mut got = vec![];
        trinity_pack(&pack, "/arc/data.trpfs#0", &mut |h, d| {
            got.push((h, d));
            Ok(())
        })
        .unwrap();
        assert_eq!(got, vec![(0x1111, b"first file".to_vec()), (0x2222, b"BNSH second".to_vec())]);
    }

    #[test]
    fn gfpak_entries() {
        let a = b"stored entry".to_vec();
        let b = b"lz4 entry lz4 entry lz4 entry".to_vec();
        let bl = lz4_flex::block::compress(&b);
        let mut d = vec![0u8; 0x40];
        d[..8].copy_from_slice(GFPAK);
        d[8..12].copy_from_slice(&0x1000u32.to_le_bytes());
        d[0x10..0x14].copy_from_slice(&2u32.to_le_bytes());
        d[0x18..0x20].copy_from_slice(&0x40u64.to_le_bytes());
        d.resize(0x40 + 0x30, 0);
        let put = |d: &mut Vec<u8>, i: usize, ct: u16, usz: usize, raw: &[u8]| {
            let off = d.len();
            d.extend_from_slice(raw);
            let e = 0x40 + 0x18 * i;
            d[e + 2..e + 4].copy_from_slice(&ct.to_le_bytes());
            d[e + 4..e + 8].copy_from_slice(&(usz as u32).to_le_bytes());
            d[e + 8..e + 12].copy_from_slice(&(raw.len() as u32).to_le_bytes());
            d[e + 0x10..e + 0x18].copy_from_slice(&(off as u64).to_le_bytes());
        };
        put(&mut d, 0, 0, a.len(), &a);
        put(&mut d, 1, 2, b.len(), &bl);
        let mut got = vec![];
        gfpak_members(&d, "/x.gfpak", &mut |i, m| got.push((i, m))).unwrap();
        assert_eq!(got, vec![(0, a), (1, b)]);
    }
}
