//! FromSoftware (Dark Souls Remastered): dvdbnd archives (.bdt) indexed by the .bhd5 beside them, DCX containers
//! and the BND3 archives inside, whose entries are GLSLC outputs.

use super::{archive, glslc};
use crate::nvn::*;

/// A BHD5 index: 'BHD5', 0xff (little-endian), u32 1, u32 file size, +0x10 u32 bucket count, u32 0, u64 bucket table
/// offset; 16-byte buckets {u32 count, u32, u64 offset} of 16-byte entries {u32 path hash, u32 size, u64 offset}.
/// → (hash, offset, size), in offset order.
pub fn bhd5_entries(d: &[u8], label: &str) -> R<Vec<(u32, u64, u64)>> {
    if d.get(..5) != Some(b"BHD5\xff") {
        return Err(format!("{label}: not a little-endian BHD5 index"));
    }
    let (nb, tab) = (need32(d, 0x10, label)? as u64, need64(d, 0x18, label)?);
    let mut out = vec![];
    for i in 0..nb {
        let b = span(d, tab + 16 * i, 16, label)?;
        let (n, at) = (u32_at(b, 0).unwrap() as u64, u64_at(b, 8).unwrap());
        for e in span(d, at, 16 * n, label)?.chunks_exact(16) {
            out.push((u32_at(e, 0).unwrap(), u64_at(e, 8).unwrap(), u32_at(e, 4).unwrap() as u64));
        }
    }
    out.sort_by_key(|e| e.1);
    Ok(out)
}

/// A DCX container (big-endian): 'DCX\0', +0x1c length, stored length, +0x28 the method (DFLT: zlib), +0x44 'DCA\0',
/// the stream at +0x4c.
pub fn undcx(d: &[u8], label: &str) -> R<Vec<u8>> {
    let be = |o: usize| d.get(o..o + 4).map(|b| u32::from_be_bytes(b.try_into().unwrap()) as usize).ok_or_else(|| format!("{label}: DCX header cut short"));
    let (n, csz) = (be(0x1c)?, be(0x20)?);
    if d.get(0x28..0x2c) != Some(b"DFLT") || d.get(0x44..0x48) != Some(b"DCA\0") {
        return Err(format!("{label}: DCX method {:?}: only DFLT is known", d.get(0x28..0x2c).map(String::from_utf8_lossy)));
    }
    let out = archive::unzlib(span(d, 0x4c, csz as u64, label)?, label)?;
    if out.len() != n {
        return Err(format!("{label}: DCX holds {:#x} bytes, its header says {n:#x}", out.len()));
    }
    Ok(out)
}

/// A BND3 archive (formats 0x5c and 0x7c, little-endian): 'BND3', 8 version bytes, u8 format, u8 big-endian, u8,
/// u8, i32 count, i32 header end, 8 bytes, then 28-byte entries {u8 flags, 3 bytes, i32 length, u64 offset, i32 ID,
/// u32 name offset, i32 unpacked length}. → (name after '/Source/', data).
pub fn bnd3_members<'a>(d: &'a [u8], label: &str) -> R<Vec<(String, &'a [u8])>> {
    if d.len() < 0x20 || !matches!(d[12], 0x5c | 0x7c) || d[13] != 0 {
        return Err(format!("{label}: BND3 format {:#x}: only formats 0x5c and 0x7c, little-endian, are known", d.get(12).copied().unwrap_or(0)));
    }
    let n = i32_at(d, 0x10).unwrap().max(0) as usize;
    let mut out = vec![];
    for i in 0..n {
        let e = span(d, 0x20 + 28 * i as u64, 28, label)?;
        let (len, off, nm, usz) = (i32_at(e, 4).unwrap(), u64_at(e, 8).unwrap(), u32_at(e, 20).unwrap() as usize, i32_at(e, 24).unwrap());
        if len != usz {
            return Err(format!("{label}: BND3 entry {i} is compressed"));
        }
        let s = d.get(nm..).unwrap_or(&[]);
        let name = String::from_utf8_lossy(&s[..memchr::memchr(0, s).unwrap_or(s.len())]).replace('\\', "/");
        let name = name.split_once("/Source/").map(|x| x.1.to_string()).unwrap_or(name);
        out.push((name, span(d, off, len.max(0) as u64, label)?));
    }
    Ok(out)
}

/// A dvdbnd entry or a loose file: DCX decoded, then the GLSLC outputs that are entries of its BND3 archive.
pub fn bnd(d: &[u8], label: &str) -> R<Vec<Found>> {
    let d = if d.starts_with(b"DCX\0") { undcx(d, label)? } else { d.to_vec() };
    if !d.starts_with(b"BND3") || memchr::memmem::find(&d, &glslc::MAGIC).is_none() {
        return Ok(vec![]);
    }
    let mut out = vec![];
    for (nm, b) in bnd3_members(&d, label)? {
        if b.starts_with(&glslc::MAGIC) {
            out.extend(glslc::shaders(b, &format!("{label}/{nm}")));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nvn::synth::*;

    #[test]
    fn bdt_dcx_bnd3() {
        let (c, k) = nvn1_ps(20);
        let g = crate::formats::glslc::tests::make_output(&c, &k);
        let mut b = b"BND3".to_vec();
        b.extend_from_slice(b"07D7R6\0\0");
        b.extend_from_slice(&[0x5c, 0, 0, 0]);
        b.extend_from_slice(&1i32.to_le_bytes());
        b.extend_from_slice(&0i32.to_le_bytes());
        b.resize(0x20, 0);
        let name_at = 0x20 + 28;
        let name = b"N:\\FRPG\\Source\\Shader\\a.fpo\0";
        let data_at = name_at + name.len();
        let mut e = vec![0u8; 4];
        e.extend_from_slice(&(g.len() as i32).to_le_bytes());
        e.extend_from_slice(&(data_at as u64).to_le_bytes());
        e.extend_from_slice(&0i32.to_le_bytes());
        e.extend_from_slice(&(name_at as u32).to_le_bytes());
        e.extend_from_slice(&(g.len() as i32).to_le_bytes());
        b.extend_from_slice(&e);
        b.extend_from_slice(name);
        b.extend_from_slice(&g);
        let mut z = flate2::write::ZlibEncoder::new(vec![], flate2::Compression::default());
        std::io::Write::write_all(&mut z, &b).unwrap();
        let z = z.finish().unwrap();
        let mut dcx = b"DCX\0".to_vec();
        dcx.resize(0x1c, 0);
        dcx.extend_from_slice(&(b.len() as u32).to_be_bytes());
        dcx.extend_from_slice(&(z.len() as u32).to_be_bytes());
        dcx.resize(0x28, 0);
        dcx.extend_from_slice(b"DFLT");
        dcx.resize(0x44, 0);
        dcx.extend_from_slice(b"DCA\0");
        dcx.extend_from_slice(&8u32.to_be_bytes());
        dcx.extend_from_slice(&z);
        // the index: one bucket of one entry
        let mut idx = b"BHD5\xff\0\0\0".to_vec();
        idx.resize(0x10, 0);
        idx.extend_from_slice(&1u32.to_le_bytes());
        idx.extend_from_slice(&0u32.to_le_bytes());
        idx.extend_from_slice(&0x20u64.to_le_bytes());
        idx.extend_from_slice(&1u32.to_le_bytes());
        idx.extend_from_slice(&0u32.to_le_bytes());
        idx.extend_from_slice(&0x30u64.to_le_bytes());
        idx.extend_from_slice(&0xabcdu32.to_le_bytes());
        idx.extend_from_slice(&(dcx.len() as u32).to_le_bytes());
        idx.extend_from_slice(&0x10u64.to_le_bytes());
        assert_eq!(bhd5_entries(&idx, "x").unwrap(), vec![(0xabcd, 0x10, dcx.len() as u64)]);
        let f = bnd(&dcx, "/dvdbnd0.bdt#0000abcd").unwrap();
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].key(), skey(&c, &k));
        assert!(f[0].label.starts_with("/dvdbnd0.bdt#0000abcd/Shader/a.fpo"));
    }
}
