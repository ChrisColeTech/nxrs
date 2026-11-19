//! Tegra block-linear <-> linear.
//!
//! GOB = 64 bytes x 8 rows = 512 bytes; offset of (x byte, y row) within a GOB on the X1 (Switch):
//!   (x%64/32)*256 + (y%8/2)*64 + (x%32/16)*32 + (y%2)*16 + x%16
//! The Switch 2 orders a GOB's 16-byte pieces otherwise:
//!   (x%64/32)*256 + (y%8/4)*128 + (x%32/16)*64 + (y%4/2)*32 + (y%2)*16 + x%16
//! A block is 1 GOB wide x 2^tileH GOBs high x 2^tileD GOBs deep, y first then z inside; blocks fill the level x, y, z
//! row-major. Level and layer offsets and each level's block shape come from `layout`.

use super::layout::{fmt_bpb, level_gobs, level_offset, level_tiles, tex_layout, TexDesc};
use std::sync::atomic::{AtomicBool, Ordering};

/// The Switch 2's order inside a GOB, for textures a Switch 2 game tiled itself. Set at nvnDeviceInitialize.
pub static GOB_SWITCH2: AtomicBool = AtomicBool::new(false);

fn gob_off(x: u32, y: u32) -> u32 {
    if GOB_SWITCH2.load(Ordering::Relaxed) {
        (x % 64 / 32) * 256 + (y % 8 / 4) * 128 + (x % 32 / 16) * 64 + (y % 4 / 2) * 32 + (y % 2) * 16 + x % 16
    } else {
        (x % 64 / 32) * 256 + (y % 8 / 2) * 64 + (x % 32 / 16) * 32 + (y % 2) * 16 + x % 16
    }
}

/// Block-linear byte offset of (x byte, y block row, z slice) inside one level and layer.
pub fn bl_offset(x: u32, y: u32, z: u32, row_bytes: u32, rows: u32, th: u32, td: u32) -> u64 {
    let bh = 1u32 << th;
    let bd = 1u32 << td;
    let gobs_x = row_bytes.div_ceil(64) as u64;
    let blocks_y = rows.div_ceil(8 * bh) as u64;
    let block = 512u64 * bh as u64 * bd as u64;
    (z / bd) as u64 * gobs_x * blocks_y * block
        + (y / (8 * bh)) as u64 * gobs_x * block
        + (x / 64) as u64 * block
        + (((z % bd) * bh + (y % (8 * bh)) / 8) as u64) * 512
        + gob_off(x, y) as u64
}

fn mip(s: u32, l: u32) -> u32 {
    let v = s >> l;
    if v == 0 { 1 } else { v }
}

/// One level and layer as stored: bytes per block row, block rows, slices, its start in the storage, block shape and
/// bytes per block. None for multisample, buffer and linear textures.
#[derive(Clone, Copy, Debug)]
pub struct LevelGeom {
    pub row_bytes: u32,
    pub rows: u32,
    pub slices: u32,
    pub base: u64,
    pub th: u32,
    pub td: u32,
    pub bpb: u32,
}

pub fn geom(d: &TexDesc, level: u32, layer: u32) -> Option<LevelGeom> {
    if d.target == 5 || d.target == 6 || d.target == 10 || (d.flags & 0x50) != 0 || d.samples > 1 {
        return None;
    }
    let (bpb, bw, bh) = fmt_bpb(d.format);
    if bpb == 0 || level >= d.levels.max(1) {
        return None;
    }
    let l = tex_layout(d);
    if layer >= l.layers {
        return None;
    }
    let one_d = d.target == 0 || d.target == 3;
    let w = mip(d.width, level);
    let h = if one_d { 1 } else { mip(d.height, level) };
    let slices = if d.target == 2 { mip(d.depth.max(1), level) } else { 1 };
    let (th, td) = level_tiles(d, level);
    Some(LevelGeom {
        row_bytes: w.div_ceil(bw) * bpb,
        rows: h.div_ceil(bh),
        slices,
        base: layer as u64 * l.layer_size + level_offset(d, level),
        th,
        td,
        bpb,
    })
}

fn mv(d: &TexDesc, level: u32, layer: u32, bl: &mut [u8], lin: &mut [u8], pitch: usize, to_linear: bool) -> Option<()> {
    let g = geom(d, level, layer)?;
    if pitch < g.row_bytes as usize {
        return None;
    }
    let mut lb = level_gobs(d, level) * 64;
    if lb < g.row_bytes {
        lb = g.row_bytes;
    }
    const KOX1: [u64; 4] = [0, 32, 256, 288];
    const KOY1: [u64; 8] = [0, 16, 64, 80, 128, 144, 192, 208];
    const KOX2: [u64; 4] = [0, 64, 256, 320];
    const KOY2: [u64; 8] = [0, 16, 32, 48, 128, 144, 160, 176];
    let s2 = GOB_SWITCH2.load(Ordering::Relaxed);
    let (kox, koy) = if s2 { (&KOX2, &KOY2) } else { (&KOX1, &KOY1) };
    let bh = 1u32 << g.th;
    let bd = 1u32 << g.td;
    let block = 512u64 * bh as u64 * bd as u64;
    let gobs_x = (lb as u64).div_ceil(64);
    let blocks_y = (g.rows as u64 + 8 * bh as u64 - 1) >> (3 + g.th);
    let base = g.base as usize;
    let nfull = (g.row_bytes >> 4) as usize;
    let tail = (g.row_bytes & 15) as usize;
    for z in 0..g.slices {
        let zb = (z >> g.td) as u64 * gobs_x * blocks_y * block + (z & (bd - 1)) as u64 * bh as u64 * 512;
        for y in 0..g.rows {
            let p = (y >> (3 + g.th)) as u64 * gobs_x * block + (((y & (8 * bh - 1)) >> 3) as u64) * 512 + koy[(y & 7) as usize];
            let row = ((z * g.rows + y) as usize) * pitch;
            let yb = zb + p;
            for j in 0..=nfull {
                let n = if j < nfull { 16 } else { tail };
                if n == 0 {
                    continue;
                }
                let src = base + (yb + (j as u64 >> 2) * block + kox[j & 3]) as usize;
                let dst = row + (j << 4);
                if src + n > bl.len() || dst + n > lin.len() {
                    return None;
                }
                if to_linear {
                    lin[dst..dst + n].copy_from_slice(&bl[src..src + n]);
                } else {
                    bl[src..src + n].copy_from_slice(&lin[dst..dst + n]);
                }
            }
        }
    }
    Some(())
}

/// Decode one level and layer of the whole-texture storage into rows of `pitch` bytes (block rows for compressed
/// formats; 3D: slice k after k x rows rows).
pub fn deswizzle(d: &TexDesc, level: u32, layer: u32, storage: &[u8], dst: &mut [u8], pitch: usize) -> Option<()> {
    // SAFETY of the cast: `mv` only reads `bl` when to_linear is set.
    let bl = unsafe { core::slice::from_raw_parts_mut(storage.as_ptr() as *mut u8, storage.len()) };
    mv(d, level, layer, bl, dst, pitch, true)
}

/// The reverse, for round-trip tests and texel writes.
pub fn swizzle(d: &TexDesc, level: u32, layer: u32, src: &[u8], pitch: usize, storage: &mut [u8]) -> Option<()> {
    let lin = unsafe { core::slice::from_raw_parts_mut(src.as_ptr() as *mut u8, src.len()) };
    mv(d, level, layer, storage, lin, pitch, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gob_offsets_by_hand() {
        // x byte 16 of row 0 is the second 16-byte piece: +32 on the X1.
        assert_eq!(bl_offset(16, 0, 0, 64, 8, 0, 0), 32);
        assert_eq!(bl_offset(0, 1, 0, 64, 8, 0, 0), 16);
        assert_eq!(bl_offset(0, 2, 0, 64, 8, 0, 0), 64);
        assert_eq!(bl_offset(32, 0, 0, 64, 8, 0, 0), 256);
        // second GOB column: one block (1 GOB high) further
        assert_eq!(bl_offset(64, 0, 0, 128, 8, 0, 0), 512);
        // block height 2: row 8 is the second GOB of the first block
        assert_eq!(bl_offset(0, 8, 0, 128, 16, 1, 0), 512);
        assert_eq!(bl_offset(64, 0, 0, 128, 16, 1, 0), 1024);
    }

    #[test]
    fn round_trip_levels_and_layers() {
        for (fmt, w, h, levels, target, depth) in [(0x25, 70, 45, 4, 1, 1), (0x42, 128, 64, 3, 1, 1), (0x25, 33, 17, 2, 4, 3), (0x01, 9, 300, 5, 1, 1)] {
            let d = TexDesc { target, format: fmt, width: w, height: h, depth, levels, ..Default::default() };
            let l = tex_layout(&d);
            let mut storage = vec![0u8; l.size as usize];
            for layer in 0..l.layers {
                for lv in 0..levels {
                    let g = geom(&d, lv, layer).unwrap();
                    let n = g.row_bytes as usize * g.rows as usize * g.slices as usize;
                    let src: Vec<u8> = (0..n).map(|i| (i as u32 * 7 + lv * 31 + layer * 13) as u8).collect();
                    swizzle(&d, lv, layer, &src, g.row_bytes as usize, &mut storage).unwrap();
                    let mut back = vec![0u8; n];
                    deswizzle(&d, lv, layer, &storage, &mut back, g.row_bytes as usize).unwrap();
                    assert_eq!(src, back, "format {fmt:#x} level {lv} layer {layer}");
                }
            }
        }
    }
}
