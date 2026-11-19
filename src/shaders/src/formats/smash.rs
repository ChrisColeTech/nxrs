//! Super Smash Bros. Ultimate's data.arc: its file-system table (one zstd frame) and the current file of each path,
//! every region's copy of a regional file. The files are scanned as files of their own (SSBH shader databases with
//! GLSLC outputs, effects and UI layouts with BNSH).
//!
//! Header: u64 magic 0xABCDEF9876543210, then u64 offsets of the music section, the file section, the shared file
//! section, the file-system table. The table: {u32 data start, u32 size, u32 stored size, u32 section size} and the
//! frame. Inflated: a 0x58-byte header (table size, path count, path-to-info index count, directory count, directory
//! offset count, child hash count, file info count, info-to-data count, file data count, second directory offset
//! count, second file data count, ..., +0x38 u8 region count, +0x3c version, extra directories, extra files, 0,
//! extra files again, extra info-to-data, ...), the regions (12 each), the stream header and tables, the path hash
//! groups and buckets, then paths (32: path, extension, parent, name, each {u32 CRC-32, u8 length, u24 index}),
//! path-to-info indices (8), directory hashes and infos (8 + 0x34), directory offsets (0x1c: u64 offset in the file
//! section, ...), child hashes (8), file infos (16: path, path-to-info, info-to-data index, flags; 0x8000 regional),
//! info-to-data (12: directory offset index, file data index, load type), file data (16: offset / 4, stored size,
//! size, flags).

use super::archive;
use crate::nvn::*;

pub const MAGIC: [u8; 8] = 0xABCD_EF98_7654_3210u64.to_le_bytes();

/// (path Hash40, absolute offset, stored size, size), in offset order, one per data.
pub fn files(read: &dyn Fn(u64, u64) -> R<Vec<u8>>, label: &str) -> R<Vec<(u64, u64, u64, u64)>> {
    let h = read(0, 0x38)?;
    if h.get(..8) != Some(&MAGIC[..]) {
        return Err(format!("{label}: not a data.arc"));
    }
    let (file_off, fs_off) = (u64_at(&h, 0x10).unwrap(), u64_at(&h, 0x20).unwrap());
    let th = read(fs_off, 16)?;
    let (size, stored) = (u32_at(&th, 4).unwrap() as usize, u32_at(&th, 8).unwrap() as u64);
    let fs = archive::unzstd(&read(fs_off + 16, stored)?, &Default::default(), label)?;
    if fs.len() != size {
        return Err(format!("{label}: the file system table inflates to {:#x} bytes, its header says {size:#x}", fs.len()));
    }
    let w = |o: usize| u32_at(&fs, o).map(|x| x as usize).ok_or_else(|| format!("{label}: the file system table is cut short at {o:#x}"));
    let (tsize, n_paths, n_idx, n_dirs, n_off1, n_hfold, n_info, n_sub, n_data, n_off2, n_data2) = (w(0)?, w(4)?, w(8)?, w(12)?, w(16)?, w(20)?, w(24)?, w(28)?, w(32)?, w(36)?, w(40)?);
    let (version, extra_dirs, extra_files, extra_f2d) = (w(0x3c)?, w(0x40)?, w(0x44)?, w(0x50)?);
    let regions = *fs.get(0x38).ok_or("cut short")? as usize;
    let mut o = 0x58 + regions * 12;
    let (qd, sh, sfi, soe) = (w(o)?, w(o + 4)?, w(o + 8)?, w(o + 12)?);
    o += 16 + qd * 12 + sh * 20 + sfi * 4 + soe * 16;
    let (groups, buckets) = (w(o)?, w(o + 4)?);
    o += 8 + buckets * 8 + groups * 8;
    let p = o;
    o += n_paths * 32;
    let idx = o;
    o += n_idx * 8 + n_dirs * (8 + 0x34);
    let dof = o;
    let n_do = n_off1 + n_off2 + extra_dirs;
    o += n_do * 0x1c + n_hfold * 8;
    let fi = o;
    let n_fi = n_info + n_data2 + extra_files;
    o += n_fi * 16;
    let f2d = o;
    let n_f2d = n_sub + n_data2 + extra_f2d;
    o += n_f2d * 12;
    let fd = o;
    let n_fd = n_data + n_data2 + extra_files;
    o += n_fd * 16;
    if o > tsize || o > fs.len() {
        return Err(format!("{label}: version {version:#x}: the file system table does not have the layout this reader knows"));
    }
    let r32 = |o: usize| u32_at(&fs, o).unwrap() as usize;
    let mut out = vec![];
    let mut seen = std::collections::HashSet::new();
    for i in 0..n_paths {
        let (c, wd) = (r32(p + 32 * i) as u64, r32(p + 32 * i + 4));
        let k = wd >> 8;
        if k >= n_idx {
            continue;
        }
        let fii = r32(idx + 8 * k + 4);
        if fii >= n_fi {
            continue;
        }
        let (f2d0, flags) = (r32(fi + 16 * fii + 8), r32(fi + 16 * fii + 12));
        let nreg = if flags & 0x8000 != 0 { regions } else { 1 };
        for f2di in f2d0..f2d0 + nreg {
            if f2di >= n_f2d {
                break;
            }
            let (doi, fdi) = (r32(f2d + 12 * f2di), r32(f2d + 12 * f2di + 4));
            if doi >= n_do || fdi >= n_fd {
                continue;
            }
            let (off_in, csz, usz) = (r32(fd + 16 * fdi) as u64, r32(fd + 16 * fdi + 4) as u64, r32(fd + 16 * fdi + 8) as u64);
            let at = file_off + u64_at(&fs, dof + 0x1c * doi).unwrap() + 4 * off_in;
            if csz == 0 || !seen.insert(at) {
                continue;
            }
            out.push((c | ((wd as u64 & 0xff) << 32), at, csz, usz));
        }
    }
    out.sort_by_key(|e| e.1);
    Ok(out)
}

/// A file's bytes, inflated when stored as a zstd frame.
pub fn read_file(read: &dyn Fn(u64, u64) -> R<Vec<u8>>, e: &(u64, u64, u64, u64), label: &str) -> R<Vec<u8>> {
    let d = read(e.1, e.2)?;
    if d.starts_with(&archive::ZSTD_MAGIC) {
        archive::unzstd(&d, &Default::default(), label)
    } else {
        Ok(d[..(e.3 as usize).min(d.len())].to_vec())
    }
}

/// A file's label: its format (an SSBH resource's type) and its path's Hash40.
pub fn label_of(arc: &str, d: &[u8], hash: u64) -> String {
    let kind: Vec<u8> = if d.starts_with(b"HBSS") && d.len() >= 0x14 { d[0x10..0x14].iter().rev().copied().collect() } else { d.get(..4).unwrap_or(&[]).to_vec() };
    let k = if kind.len() == 4 && kind.iter().all(|c| c.is_ascii_alphanumeric()) { String::from_utf8_lossy(&kind).into_owned() } else { "data".into() };
    format!("{arc}/{k}#{hash:010x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table() {
        // one path, one file info, one info-to-data, one file data; no regions or streams
        let mut fs = vec![0u8; 0x58];
        let set = |fs: &mut Vec<u8>, o: usize, v: u32| fs[o..o + 4].copy_from_slice(&v.to_le_bytes());
        for (o, v) in [(4, 1u32), (8, 1), (16, 1), (24, 1), (28, 1), (32, 1)] {
            set(&mut fs, o, v);
        }
        fs.extend_from_slice(&[0; 16 + 8]); // stream header, hash groups
        let mut path = vec![0u8; 32];
        path[..4].copy_from_slice(&0xdeadbeefu32.to_le_bytes());
        path[4..8].copy_from_slice(&(0x0000_0011u32).to_le_bytes()); // length 0x11, index 0
        fs.extend_from_slice(&path);
        fs.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]); // path-to-info: file info 0
        let mut dofs = vec![0u8; 0x1c];
        dofs[..8].copy_from_slice(&0x100u64.to_le_bytes());
        fs.extend_from_slice(&dofs);
        fs.extend_from_slice(&[0; 16]); // file info: info-to-data 0, flags 0
        fs.extend_from_slice(&[0; 12]); // info-to-data: directory offset 0, file data 0
        let mut fdat = vec![];
        for x in [4u32, 9, 9, 0] {
            fdat.extend_from_slice(&x.to_le_bytes());
        }
        fs.extend_from_slice(&fdat);
        let n = fs.len() as u32;
        set(&mut fs, 0, n);
        let z = zstd::bulk::compress(&fs, 3).unwrap();
        let mut arc = MAGIC.to_vec();
        arc.resize(0x38, 0);
        arc[0x10..0x18].copy_from_slice(&0x40u64.to_le_bytes());
        let fs_off = 0x200u64;
        arc[0x20..0x28].copy_from_slice(&fs_off.to_le_bytes());
        arc.resize(0x150, 0);
        arc.extend_from_slice(b"shaderdb!");
        arc.resize(fs_off as usize, 0);
        for x in [0x10u32, fs.len() as u32, z.len() as u32, 0] {
            arc.extend_from_slice(&x.to_le_bytes());
        }
        arc.extend_from_slice(&z);
        let read = |o: u64, n: u64| Ok(arc[o as usize..(o + n) as usize].to_vec());
        let f = files(&read, "/data.arc").unwrap();
        assert_eq!(f, vec![(0x11_deadbeef, 0x150, 9, 9)]);
        assert_eq!(read_file(&read, &f[0], "x").unwrap(), b"shaderdb!");
    }
}
