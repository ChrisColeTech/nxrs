//! A code module's image (NSO): its three segments (text, read-only data, data) at their memory offsets, each LZ4 or
//! ZBIC compressed when its flag bit says so (Switch 2 modules use ZBIC). Shaders and GLSLC outputs live in the
//! read-only data.

use super::zbic;
use crate::nvn::*;

pub fn image(nso: &[u8], label: &str) -> R<Vec<u8>> {
    if nso.len() < 0x100 || &nso[..4] != b"NSO0" {
        return Err(format!("{label}: not an NSO"));
    }
    let flags = u32_at(nso, 0xc).unwrap();
    let mut segs = vec![];
    let mut size = 0usize;
    for i in 0..3 {
        let (off, mem, len) = (
            u32_at(nso, 0x10 + 0x10 * i).unwrap() as u64,
            u32_at(nso, 0x14 + 0x10 * i).unwrap() as usize,
            u32_at(nso, 0x18 + 0x10 * i).unwrap() as usize,
        );
        let stored = u32_at(nso, 0x60 + 4 * i).unwrap() as u64;
        let compressed = flags >> i & 1 != 0;
        let raw = span(
            nso,
            off,
            if compressed { stored } else { len as u64 },
            &format!("{label} segment {i}"),
        )?;
        let data = if !compressed {
            raw.to_vec()
        } else if raw.starts_with(&zbic::MAGIC) {
            zbic::decompress(raw, len).map_err(|e| format!("{label} segment {i}: {e}"))?
        } else {
            lz4_flex::block::decompress(raw, len)
                .map_err(|e| format!("{label} segment {i}: LZ4: {e}"))?
        };
        size = size.max(mem + len);
        segs.push((mem, data));
    }
    if size > 1 << 31 {
        return Err(format!("{label}: an image of {size:#x} bytes"));
    }
    let mut img = vec![0u8; size];
    for (mem, d) in segs {
        img[mem..mem + d.len()].copy_from_slice(&d);
    }
    Ok(img)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lz4_and_plain_segments() {
        let text = vec![0x11u8; 0x300];
        let ro = b"BNSH somewhere in rodata".repeat(20);
        let lz = lz4_flex::block::compress(&ro);
        let mut n = vec![0u8; 0x100];
        n[..4].copy_from_slice(b"NSO0");
        n[0xc..0x10].copy_from_slice(&2u32.to_le_bytes());
        let segs = [
            (0x100usize, 0usize, text.len(), text.len()),
            (0x400, 0x1000, ro.len(), lz.len()),
            (0x400 + lz.len(), 0x2000, 0, 0),
        ];
        for (i, (off, mem, len, stored)) in segs.iter().enumerate() {
            n[0x10 + 0x10 * i..0x14 + 0x10 * i].copy_from_slice(&(*off as u32).to_le_bytes());
            n[0x14 + 0x10 * i..0x18 + 0x10 * i].copy_from_slice(&(*mem as u32).to_le_bytes());
            n[0x18 + 0x10 * i..0x1c + 0x10 * i].copy_from_slice(&(*len as u32).to_le_bytes());
            n[0x60 + 4 * i..0x64 + 4 * i].copy_from_slice(&(*stored as u32).to_le_bytes());
        }
        n.extend_from_slice(&text);
        n.extend_from_slice(&lz);
        let img = image(&n, "main").unwrap();
        assert_eq!(&img[..0x300], &text[..]);
        assert_eq!(&img[0x1000..0x1000 + ro.len()], &ro[..]);
    }
}
