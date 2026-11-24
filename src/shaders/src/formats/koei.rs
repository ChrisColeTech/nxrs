//! Koei Tecmo: KTGL's RDB data files (IDRK entries, PDRK .fdata files) with their G2S / _DOK containers of GLSLC
//! outputs, the LINKDATA archives and their indexes, the older chunked zlib (Fire Emblem: Three Houses) and Gust's
//! (Atelier Ryza DX).

use super::{archive, glslc};
use crate::nvn::*;

pub const IDRK: &[u8; 8] = b"IDRK0000";
pub const PDRK: &[u8; 8] = b"PDRK0000";

/// The IDRK entries of an RDB file of `size` bytes (`read(offset, length)`): 'IDRK0000', u64 entry length, u64
/// stored length, u64 length, ..., +0x2c flags; the stored data ends the entry. Entries follow each other, some
/// after padding. → (start of the stored data, its end, flags).
pub fn idrk_entries(
    read: &dyn Fn(u64, u64) -> R<Vec<u8>>,
    size: u64,
    label: &str,
) -> R<Vec<(u64, u64, u32)>> {
    let mut out = vec![];
    let mut o = 0u64;
    while o + 0x30 <= size {
        let h = read(o, 0x30)?;
        if &h[..8] != IDRK {
            // padding: find the next entry in the following 64 KB
            let n = (size - o).min(0x10000 + 8);
            let w = read(o, n)?;
            match memchr::memmem::find(&w[1.min(w.len())..], IDRK) {
                Some(p) => {
                    o += p as u64 + 1;
                    continue;
                }
                None if n < size - o => {
                    o += n - 8;
                    continue;
                }
                None => break,
            }
        }
        let (esz, csz) = (u64_at(&h, 8).unwrap(), u64_at(&h, 16).unwrap());
        if esz < 0x30 || csz > esz || o + esz > size {
            return Err(format!(
                "{label}: IDRK entry at {o:#x} of {esz:#x} bytes does not fit"
            ));
        }
        out.push((o + esz - csz, o + esz, u32_at(&h, 0x2c).unwrap()));
        o += esz;
    }
    Ok(out)
}

/// An entry's data: flag 0x100000 alone = chunks {u32 stored length, zlib stream}; 0x400000 = chunks {u16 stored
/// length, 8 bytes, stream} of LZ4 blocks (with 0x100000 too) or zlib; a zero length ends; each chunk inflates to at
/// most 16 KB. Anything else is stored. `head`: only the first bytes.
pub fn idrk_data(d: &[u8], flags: u32, head: Option<usize>, label: &str) -> R<Vec<u8>> {
    let (new, lz4b) = (flags & 0x400000 != 0, flags & 0x500000 == 0x500000);
    if flags & 0x500000 == 0 {
        return Ok(d[..head.unwrap_or(d.len()).min(d.len())].to_vec());
    }
    let mut out = vec![];
    let mut a = 0;
    while a + 4 <= d.len() {
        let cs = if new {
            u16_at(d, a).unwrap() as usize
        } else {
            u32_at(d, a).unwrap() as usize
        };
        a += if new { 10 } else { 4 };
        if cs == 0 {
            break;
        }
        let c = d
            .get(a..a + cs)
            .ok_or_else(|| format!("{label}: chunk at {a:#x} runs past the entry"))?;
        let r = if lz4b {
            lz4_flex::block::decompress(c, 0x4000).map_err(|e| format!("{label}: LZ4: {e}"))?
        } else {
            archive::unzlib(c, label)?
        };
        out.extend(r);
        if head.is_some_and(|h| out.len() >= h) {
            out.truncate(head.unwrap());
            return Ok(out);
        }
        a += cs;
    }
    Ok(out)
}

/// An RDB file's GLSLC outputs: entries whose data starts '_S2G' (G2S containers) or '_DOK' (system resources).
pub fn rdb(
    read: &dyn Fn(u64, u64) -> R<Vec<u8>>,
    size: u64,
    label: &str,
    out: &mut super::Scanned,
) {
    let ents = match idrk_entries(read, size, label) {
        Ok(e) => e,
        Err(e) => {
            out.problems.push(e);
            return;
        }
    };
    for (a, b, flags) in ents {
        let w = format!("{label}#IDRK@{a:#x}");
        let r = read(a, b - a).and_then(|raw| {
            let h = idrk_data(&raw, flags, Some(4), &w)?;
            if h != b"_S2G" && h != b"_DOK" {
                return Ok(vec![]);
            }
            Ok(glslc::shaders(&idrk_data(&raw, flags, None, &w)?, &w))
        });
        match r {
            Ok(f) => out.found.extend(f),
            Err(e) => out.problems.push(e),
        }
    }
}

/// Koei Tecmo's older chunked zlib: u32 chunk size (a power of two, 0x10000 or more), u32 chunk count, u32 length,
/// each chunk's stored length, then the chunks at 0x80 alignment, each {u32 length, zlib stream}, or stored when its
/// stored length is its inflated length. None when `d` is not in that form.
pub fn kt_gz(d: &[u8]) -> Option<Vec<u8>> {
    let (cs, n, tot) = (
        u32_at(d, 0)? as usize,
        u32_at(d, 4)? as usize,
        u32_at(d, 8)? as usize,
    );
    if cs < 0x10000
        || !cs.is_power_of_two()
        || n == 0
        || n != tot.div_ceil(cs)
        || 12 + 4 * n > d.len()
    {
        return None;
    }
    let mut o = (12 + 4 * n + 0x7f) & !0x7f;
    let mut out = Vec::with_capacity(tot);
    for i in 0..n {
        let s = u32_at(d, 12 + 4 * i)? as usize;
        let c = d.get(o..o + s)?;
        if s == cs.min(tot - i * cs) {
            out.extend_from_slice(c);
        } else {
            let ln = u32_at(c, 0)? as usize;
            out.extend(archive::unzlib(c.get(4..4 + ln)?, "").ok()?);
        }
        o = (o + s + 0x7f) & !0x7f;
    }
    (out.len() == tot).then_some(out)
}

/// Gust's chunked zlib: {u32 stored length, one zlib stream} until a zero length that ends the file. None when `d` is
/// not in that form.
pub fn gust_gz(d: &[u8]) -> Option<Vec<u8>> {
    let mut o = 0;
    let mut out = vec![];
    while o + 4 <= d.len() {
        let n = u32_at(d, o)? as usize;
        o += 4;
        if n == 0 {
            return (!out.is_empty() && o == d.len()).then_some(out);
        }
        let c = d.get(o..o + n)?;
        if c[0] != 0x78 {
            return None;
        }
        let mut z = flate2::Decompress::new(true);
        let mut buf = vec![0u8; 0x40000];
        let st = z
            .decompress(c, &mut buf, flate2::FlushDecompress::Finish)
            .ok()?;
        if st != flate2::Status::StreamEnd || z.total_in() as usize != n {
            return None;
        }
        out.extend_from_slice(&buf[..z.total_out() as usize]);
        o += n;
    }
    None
}

/// A LINKDATA archive's index beside it (by name): Three Houses' DATA0.bin beside DATA1.bin, LI_*.IDX beside
/// LD_*.BIN, LINKDATA.IDX beside LINKDATA.BIN.
pub fn linkdata_index(path: &str) -> Option<String> {
    let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
    let idx = if name == "DATA1.bin" {
        "DATA0.bin".to_string()
    } else if name == "LINKDATA.BIN" {
        "LINKDATA.IDX".to_string()
    } else if name.starts_with("LD_") && name.ends_with(".BIN") {
        format!("LI_{}.IDX", &name[3..name.len() - 4])
    } else {
        return None;
    };
    Some(if dir.is_empty() {
        idx
    } else {
        format!("{dir}/{idx}")
    })
}

/// A LINKDATA index: 32-byte entries {u64 offset, u64 inflated length, u64 stored length, u64 compressed}.
pub fn linkdata_entries(idx: &[u8]) -> Vec<(u64, u64, u64, u64)> {
    idx.chunks_exact(32)
        .map(|e| {
            (
                u64_at(e, 0).unwrap(),
                u64_at(e, 8).unwrap(),
                u64_at(e, 16).unwrap(),
                u64_at(e, 24).unwrap(),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nvn::synth::*;

    fn zlib(d: &[u8]) -> Vec<u8> {
        let mut z = flate2::write::ZlibEncoder::new(vec![], flate2::Compression::default());
        std::io::Write::write_all(&mut z, d).unwrap();
        z.finish().unwrap()
    }

    #[test]
    fn rdb_entries() {
        let (c, k) = nvn1_ps(19);
        let mut g2s = b"_S2G3000".to_vec();
        g2s.extend(crate::formats::glslc::tests::make_output(&c, &k));
        // one zlib chunk
        let z = zlib(&g2s);
        let mut data = (z.len() as u32).to_le_bytes().to_vec();
        data.extend_from_slice(&z);
        data.extend_from_slice(&0u32.to_le_bytes());
        let mut e = IDRK.to_vec();
        let esz = 0x30 + data.len() as u64;
        for x in [esz, data.len() as u64, g2s.len() as u64] {
            e.extend_from_slice(&x.to_le_bytes());
        }
        e.resize(0x2c, 0);
        e.extend_from_slice(&0x100000u32.to_le_bytes());
        e.extend_from_slice(&data);
        let mut f = vec![0u8; 0x20]; // padding before the entry
        f.extend_from_slice(&e);
        let read = |o: u64, n: u64| Ok(f[o as usize..(o + n) as usize].to_vec());
        let mut s = crate::formats::Scanned::default();
        rdb(&read, f.len() as u64, "/asset/x.rdb.bin", &mut s);
        assert!(s.problems.is_empty(), "{:?}", s.problems);
        assert_eq!(s.found.len(), 1);
        assert_eq!(s.found[0].key(), skey(&c, &k));
    }

    #[test]
    fn chunked_zlib() {
        let data: Vec<u8> = (0..0x18000u32).map(|i| (i % 13) as u8).collect();
        // Koei Tecmo's: chunks of 0x10000
        let mut d = vec![];
        for x in [0x10000u32, 2, data.len() as u32] {
            d.extend_from_slice(&x.to_le_bytes());
        }
        let chunks: Vec<Vec<u8>> = data
            .chunks(0x10000)
            .map(|c| {
                let z = zlib(c);
                let mut v = (z.len() as u32).to_le_bytes().to_vec();
                v.extend(z);
                v
            })
            .collect();
        for c in &chunks {
            d.extend_from_slice(&(c.len() as u32).to_le_bytes());
        }
        for c in &chunks {
            d.resize((d.len() + 0x7f) & !0x7f, 0);
            d.extend_from_slice(c);
        }
        assert_eq!(kt_gz(&d).unwrap(), data);
        // Gust's
        let mut g = vec![];
        for c in data.chunks(0x4000) {
            let z = zlib(c);
            g.extend_from_slice(&(z.len() as u32).to_le_bytes());
            g.extend(z);
        }
        g.extend_from_slice(&0u32.to_le_bytes());
        assert_eq!(gust_gz(&g).unwrap(), data);
        assert_eq!(
            linkdata_index("nx/DATA1.bin").as_deref(),
            Some("nx/DATA0.bin")
        );
    }
}
