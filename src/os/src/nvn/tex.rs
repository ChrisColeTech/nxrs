//! NVN texture storage sizes (Tegra X1 block-linear layout, the deko3d algorithm as nxnative's `nvn_tex.c` has
//! it): what `nvnTextureBuilderGetStorageSize` / `GetStorageAlignment` answer, so a game sizes its texture
//! memory pools as on the console.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Desc {
    pub target: i32,
    pub format: i32,
    pub flags: i32,
    pub samples: i32,
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub levels: u32,
    pub stride: u32,
    /// Packaged layout byte 0 | (byte 1 & 0xF) << 8, + 1; 0 = automatic.
    pub pkg_layout: i32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Layout {
    pub size: u64,
    pub layer_size: u64,
    pub align: u32,
    pub layers: u32,
    pub bpb: u32,
    pub tile_h: u32,
    pub tile_d: u32,
}

#[derive(Clone, Copy)]
struct Fmt {
    bpb: u32,
    bw: u32,
    bh: u32,
}

fn fmt_of(f: i32) -> Fmt {
    const ASTC: [(u32, u32); 14] = [(4, 4), (5, 4), (5, 5), (6, 5), (6, 6), (8, 5), (8, 6), (8, 8), (10, 5), (10, 6), (10, 8), (10, 10), (12, 10), (12, 12)];
    if (0x79..=0x94).contains(&f) {
        let (bw, bh) = ASTC[((f - 0x79) % 14) as usize];
        return Fmt { bpb: 16, bw, bh };
    }
    if (0x41..=0x50).contains(&f) {
        let b8 = f <= 0x42 || f == 0x45 || f == 0x46 || f == 0x49 || f == 0x4A;
        return Fmt { bpb: if b8 { 8 } else { 16 }, bw: 4, bh: 4 };
    }
    const R: &[(i32, i32, u32)] = &[
        (0x01, 0x04, 1), (0x05, 0x09, 2), (0x0A, 0x0C, 4), (0x0D, 0x10, 2), (0x11, 0x15, 4), (0x16, 0x18, 8),
        (0x19, 0x1C, 3), (0x1D, 0x21, 6), (0x22, 0x24, 12), (0x25, 0x28, 4), (0x29, 0x2D, 8), (0x2E, 0x30, 16),
        (0x31, 0x31, 1), (0x32, 0x32, 2), (0x33, 0x35, 4), (0x36, 0x36, 8), (0x37, 0x38, 4), (0x39, 0x3C, 2), (0x3D, 0x40, 4),
        (0x51, 0x52, 1), (0x53, 0x54, 2), (0x55, 0x56, 4), (0x57, 0x58, 2), (0x59, 0x5A, 4), (0x5B, 0x5C, 8),
        (0x5D, 0x5E, 3), (0x5F, 0x60, 6), (0x61, 0x62, 12), (0x63, 0x64, 4), (0x65, 0x66, 8), (0x67, 0x68, 16),
        (0x69, 0x6C, 4), (0x6D, 0x70, 4), (0x71, 0x75, 8), (0x76, 0x78, 16), (0x95, 0x98, 2), (0x99, 0x9C, 4),
    ];
    for &(lo, hi, bpb) in R {
        if f >= lo && f <= hi {
            return Fmt { bpb, bw: 1, bh: 1 };
        }
    }
    Fmt { bpb: 0, bw: 1, bh: 1 }
}

/// Bytes per block of a format (0: unknown).
pub fn bpb(format: i32) -> u32 {
    fmt_of(format).bpb
}

fn mip(s: u32, l: u32) -> u32 {
    (s >> l).max(1)
}
fn blk(s: u32, b: u32) -> u32 {
    s.div_ceil(b)
}
fn pick_tile(gobs: u32) -> u32 {
    match gobs {
        g if g >= 16 => 4,
        g if g >= 8 => 3,
        g if g >= 4 => 2,
        g if g >= 2 => 1,
        _ => 0,
    }
}
fn packaged_width_log2(d: &Desc) -> u32 {
    if d.pkg_layout != 0 {
        ((d.pkg_layout - 1) as u32 >> 8) & 0xF
    } else {
        0
    }
}
fn adj_tile(mut shift: u32, unit: u32, dim: u32) -> u32 {
    if shift == 0 {
        return 0;
    }
    let mut x = unit << (shift - 1);
    if x >= dim {
        loop {
            shift -= 1;
            if shift == 0 {
                break;
            }
            x >>= 1;
            if x < dim {
                break;
            }
        }
    }
    shift
}

/// (dims, layered, w, h, z)
fn shape(d: &Desc) -> (u32, bool, u32, u32, u32) {
    let (mut w, mut h, mut z, mut layered) = (d.width, d.height, d.depth, false);
    let dpl = match d.target {
        0 => {
            h = 1;
            z = 1;
            1
        }
        3 => {
            z = d.height;
            h = 1;
            layered = true;
            1
        }
        2 => 3,
        4 | 6 => {
            layered = true;
            2
        }
        8 => {
            z = 6;
            layered = true;
            2
        }
        9 => {
            layered = true;
            2
        }
        _ => {
            z = 1;
            2
        }
    };
    let _ = &mut w;
    (dpl, layered, w, h, z.max(1))
}

fn samples_xy(s: i32) -> (u32, u32) {
    match s {
        2 => (2, 1),
        4 => (2, 2),
        8 => (4, 2),
        _ => (1, 1),
    }
}

const SPARSE_TILE: u64 = 0x10000;
fn sparse_of(d: &Desc) -> bool {
    (d.flags & 0x20) != 0 && (d.flags & 0x50) == 0 && d.target != 10
}
fn sparse_tile_h(bpb: u32) -> i32 {
    match bpb {
        1 => 5,
        2 | 4 => 4,
        8 | 16 => 3,
        _ => -1,
    }
}
fn sparse_tile_h3(bpb: u32) -> i32 {
    match bpb {
        1 | 2 | 4 => 2,
        8 | 16 => 1,
        _ => -1,
    }
}
fn sparse_tile_d3(bpb: u32) -> i32 {
    match bpb {
        1 | 2 => 5,
        4 | 8 | 16 => 4,
        _ => -1,
    }
}
fn tile_shape_h(d: &Desc, dpl: u32, t: Fmt, tile_h: u32) -> u32 {
    if dpl == 2 && d.pkg_layout == 0 && sparse_tile_h(t.bpb) >= 0 {
        sparse_tile_h(t.bpb) as u32
    } else {
        tile_h
    }
}

#[allow(clippy::too_many_arguments)]
fn level_offset(d: &Desc, level: u32, dpl: u32, w: u32, h: u32, z: u32, tile_h: u32, tile_d: u32, t: Fmt) -> u64 {
    let (sx, sy) = samples_xy(d.samples);
    let sh = tile_shape_h(d, dpl, t, tile_h);
    let pw = packaged_width_log2(d);
    let mut tw = if pw != 0 { 1u32 << pw } else { 128u32 >> (sh + tile_d) };
    if tw == 0 {
        tw = 1;
    }
    let mut tiled = sparse_of(d) || pw != 0;
    let mut off = 0u64;
    let mut i = 0;
    loop {
        let lw = blk(mip(w * sx, i), t.bw);
        let lh = if dpl >= 2 { blk(mip(h * sy, i), t.bh) } else { 1 };
        let ld = if dpl >= 3 { mip(z, i) } else { 1 };
        let lwb = lw * t.bpb;
        let th = adj_tile(tile_h, 8, lh);
        let td = adj_tile(tile_d, 1, ld);
        let mut wg = lwb.div_ceil(64);
        let hg = lh.div_ceil(8);
        tiled = tiled && lwb >= tw * 64 && lh >= 8u32 << sh && ld >= 1u32 << tile_d;
        if tiled {
            wg = wg.div_ceil(tw) * tw;
        }
        if i == level {
            return off;
        }
        let ht = (hg + (1u32 << th) - 1) >> th;
        let dt = (ld + (1u32 << td) - 1) >> td;
        off += (wg as u64 * ht as u64 * dt as u64) << (9 + th + td);
        i += 1;
    }
}

#[allow(clippy::too_many_arguments)]
fn layer_bytes(d: &Desc, dpl: u32, layered: bool, w: u32, h: u32, z: u32, tile_h: u32, tile_d: u32, t: Fmt) -> u64 {
    let mut ls = level_offset(d, d.levels.max(1), dpl, w, h, z, tile_h, tile_d, t);
    if sparse_of(d) {
        ls = (ls + SPARSE_TILE - 1) & !(SPARSE_TILE - 1);
    }
    if layered {
        let hh = blk(h, t.bh);
        let (mut th, mut td) = (tile_h, tile_d);
        while th != 0 && hh <= (8u32 << (th - 1)) {
            th -= 1;
        }
        while td != 0 && z <= (1u32 << (td - 1)) {
            td -= 1;
        }
        let pw = packaged_width_log2(d);
        let sh = if pw != 0 { pw + tile_h + tile_d + 9 } else { th + td + 9 };
        let g = ls >> sh;
        if ls != g << sh {
            ls = (g + 1) << sh;
        }
    }
    ls
}

pub fn layout(d: &Desc) -> Layout {
    let t = fmt_of(d.format);
    let (dpl, layered, w, h, z) = shape(d);
    let (sx, sy) = samples_xy(d.samples);
    let mut o = Layout { bpb: t.bpb, layers: if layered { z } else { 1 }, ..Default::default() };
    if t.bpb == 0 {
        o.align = 512;
        return o;
    }
    if d.target == 10 {
        o.size = t.bpb as u64 * d.width as u64;
        o.layer_size = o.size;
        o.align = if t.bpb == 12 { 4 } else { t.bpb };
        return o;
    }
    if d.flags & 0x50 != 0 {
        o.align = if d.flags & 0x40 != 0 { 128 } else { 32 };
        let stride = if d.stride != 0 { d.stride } else { (t.bpb * blk(w, t.bw) + o.align - 1) & !(o.align - 1) };
        o.size = stride as u64 * h as u64;
        o.layer_size = o.size;
        return o;
    }
    let (mut tile_h, mut tile_d) = (0u32, 0u32);
    if d.pkg_layout != 0 {
        let pl = ((d.pkg_layout - 1) as u32) & 0xFF;
        if dpl >= 2 {
            tile_h = pl & 0xF;
        }
        if dpl == 3 {
            tile_d = pl >> 4;
        }
    } else if dpl == 2
        && sparse_of(d)
        && sparse_tile_h(t.bpb) >= 0
        && blk(w * sx, t.bw) * t.bpb >= (128u32 >> sparse_tile_h(t.bpb)) * 64
        && blk(h * sy, t.bh) >= 8u32 << sparse_tile_h(t.bpb)
    {
        tile_h = sparse_tile_h(t.bpb) as u32;
    } else if dpl == 3
        && sparse_of(d)
        && sparse_tile_h3(t.bpb) >= 0
        && blk(w * sx, t.bw) * t.bpb >= (128u32 >> (sparse_tile_h3(t.bpb) + sparse_tile_d3(t.bpb))) * 64
        && blk(h * sy, t.bh) >= 8u32 << sparse_tile_h3(t.bpb)
        && z >= 1u32 << sparse_tile_d3(t.bpb)
    {
        tile_h = sparse_tile_h3(t.bpb) as u32;
        tile_d = sparse_tile_d3(t.bpb) as u32;
    } else if dpl == 2 {
        let ah = blk(h * sy, t.bh);
        tile_h = pick_tile(ah * 3 / 16);
    } else if dpl == 3 {
        tile_d = pick_tile(z + z / 2);
    }
    let mut ls = layer_bytes(d, dpl, layered, w, h, z, tile_h, tile_d, t);
    if d.flags & 0x200 != 0 && d.pkg_layout == 0 && !sparse_of(d) && (tile_h != 0 || tile_d != 0) {
        let one = layer_bytes(d, dpl, layered, w, h, z, 0, 0, t);
        if one != ls {
            tile_h = 0;
            tile_d = 0;
            ls = one;
        }
    }
    o.tile_h = tile_h;
    o.tile_d = tile_d;
    o.layer_size = ls;
    o.size = if layered { ls * z as u64 } else { ls };
    o.align = if sparse_of(d) { SPARSE_TILE as u32 } else { 512 };
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d2(format: i32, w: u32, h: u32, levels: u32) -> Desc {
        Desc { target: 1, format, width: w, height: h, depth: 1, levels, ..Default::default() }
    }

    #[test]
    fn sizes() {
        // RGBA8 (0x25) 256x256, one level: 1024 bytes a row = 16 GOBs wide, 32 GOB rows, block height 16 -> 2 blocks
        let l = layout(&d2(0x25, 256, 256, 1));
        assert_eq!((l.size, l.align, l.bpb, l.tile_h), (256 * 256 * 4, 512, 4, 4));
        // a 1280x720 RGBA8 render target
        let l = layout(&d2(0x25, 1280, 720, 1));
        assert_eq!(l.size % 512, 0);
        assert!(l.size >= 1280 * 720 * 4 && l.size < 1280 * 720 * 4 * 2);
        // linear with an explicit stride
        let l = layout(&Desc { target: 1, format: 0x25, flags: 0x40, width: 16, height: 16, depth: 1, levels: 1, stride: 128, ..Default::default() });
        assert_eq!((l.size, l.align), (2048, 128));
        // BC1 (0x42) 4x4 blocks of 8 bytes, with mips
        let l = layout(&d2(0x42, 128, 128, 8));
        assert!(l.size >= 128 * 128 / 2);
        // a buffer texture
        let l = layout(&Desc { target: 10, format: 0x25, width: 100, ..Default::default() });
        assert_eq!((l.size, l.align), (400, 4));
        // a cube map: six layers
        let l = layout(&Desc { target: 8, format: 0x25, width: 64, height: 64, depth: 1, levels: 1, ..Default::default() });
        assert_eq!(l.layers, 6);
        assert_eq!(l.size, l.layer_size * 6);
        assert_eq!(layout(&d2(0x7FFF, 4, 4, 1)).size, 0, "unknown format");
    }
}
