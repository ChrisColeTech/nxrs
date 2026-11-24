//! NVN texture storage layout (Tegra X1 block-linear): storage size, alignment, per-level offsets.
//!
//! One GOB is 64 bytes x 8 rows = 512 bytes; the block height (tileH, in GOBs, log2) is picked from the height in blocks
//! x 1.5 / 8; levels are accumulated with block alignment. A game's packaged textures (SetPackagedTextureLayout) carry
//! 8 bytes of layout parameters: byte 0's low nibble = log2 of the block height, its high nibble = log2 of the block
//! depth (3D); byte 1's low nibble = log2 of the width alignment in GOBs. Linear storage counts pixel rows, also for
//! block-compressed formats.

/// The texture as the record layer keeps it (`tex_t.d`). 40 bytes, part of the in-place texture object.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct TexDesc {
    pub target: i32,
    pub format: i32,
    pub flags: i32,
    /// sample count (0 or 1 = single-sampled)
    pub samples: i32,
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub levels: u32,
    pub stride: u32,
    /// packaged layout byte 0 | byte 1's low nibble << 8, + 1; 0 = automatic
    pub pkg_layout: i32,
}

/// What a texture's storage looks like. 40 bytes, part of the in-place texture object.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct TexLayout {
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
    const ASTC: [[u32; 2]; 14] = [
        [4, 4],
        [5, 4],
        [5, 5],
        [6, 5],
        [6, 6],
        [8, 5],
        [8, 6],
        [8, 8],
        [10, 5],
        [10, 6],
        [10, 8],
        [10, 10],
        [12, 10],
        [12, 12],
    ];
    if (0x79..=0x94).contains(&f) {
        let k = ((f - 0x79) % 14) as usize;
        return Fmt {
            bpb: 16,
            bw: ASTC[k][0],
            bh: ASTC[k][1],
        };
    }
    if (0x41..=0x50).contains(&f) {
        let b8 = f <= 0x42 || f == 0x45 || f == 0x46 || f == 0x49 || f == 0x4A;
        return Fmt {
            bpb: if b8 { 8 } else { 16 },
            bw: 4,
            bh: 4,
        };
    }
    const R: &[(i32, i32, u32)] = &[
        (0x01, 0x04, 1),
        (0x05, 0x09, 2),
        (0x0A, 0x0C, 4),
        (0x0D, 0x10, 2),
        (0x11, 0x15, 4),
        (0x16, 0x18, 8),
        (0x19, 0x1C, 3),
        (0x1D, 0x21, 6),
        (0x22, 0x24, 12),
        (0x25, 0x28, 4),
        (0x29, 0x2D, 8),
        (0x2E, 0x30, 16),
        (0x31, 0x31, 1),
        (0x32, 0x32, 2),
        (0x33, 0x35, 4),
        (0x36, 0x36, 8),
        (0x37, 0x38, 4),
        (0x39, 0x3C, 2),
        (0x3D, 0x40, 4),
        (0x51, 0x52, 1),
        (0x53, 0x54, 2),
        (0x55, 0x56, 4),
        (0x57, 0x58, 2),
        (0x59, 0x5A, 4),
        (0x5B, 0x5C, 8),
        (0x5D, 0x5E, 3),
        (0x5F, 0x60, 6),
        (0x61, 0x62, 12),
        (0x63, 0x64, 4),
        (0x65, 0x66, 8),
        (0x67, 0x68, 16),
        (0x69, 0x6C, 4),
        (0x6D, 0x70, 4),
        (0x71, 0x75, 8),
        (0x76, 0x78, 16),
        (0x95, 0x98, 2),
        (0x99, 0x9C, 4),
    ];
    for &(lo, hi, bpb) in R {
        if f >= lo && f <= hi {
            return Fmt { bpb, bw: 1, bh: 1 };
        }
    }
    Fmt {
        bpb: 0,
        bw: 1,
        bh: 1,
    }
}

/// Bytes per block (0 = unknown format), block width and height.
pub fn fmt_bpb(format: i32) -> (u32, u32, u32) {
    let t = fmt_of(format);
    (t.bpb, t.bw, t.bh)
}

fn mip(s: u32, l: u32) -> u32 {
    let v = s >> l;
    if v == 0 {
        1
    } else {
        v
    }
}
fn blk(s: u32, b: u32) -> u32 {
    s.div_ceil(b)
}
fn pick_tile(gobs: u32) -> u32 {
    if gobs >= 16 {
        4
    } else if gobs >= 8 {
        3
    } else if gobs >= 4 {
        2
    } else if gobs >= 2 {
        1
    } else {
        0
    }
}
fn packaged_width_log2(d: &TexDesc) -> u32 {
    if d.pkg_layout != 0 {
        ((d.pkg_layout - 1) as u32 >> 8) & 0xf
    } else {
        0
    }
}
/// The block height a level of `dim` units actually uses (the per-level block height shrinks with its rows).
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

/// Dimensions per layer and whether the target is layered.
fn shape(d: &TexDesc) -> (u32, bool, u32, u32, u32) {
    let (mut w, mut h, mut z) = (d.width, d.height, d.depth);
    let mut layered = false;
    let dpl;
    match d.target {
        0 => {
            dpl = 1;
            h = 1;
            z = 1;
        }
        3 => {
            dpl = 1;
            z = d.height;
            h = 1;
            layered = true;
        }
        2 => dpl = 3,
        4 | 6 => {
            dpl = 2;
            layered = true;
        }
        8 => {
            dpl = 2;
            z = 6;
            layered = true;
        }
        9 => {
            dpl = 2;
            layered = true;
        }
        _ => {
            dpl = 2;
            z = 1;
        }
    }
    if z == 0 {
        z = 1;
    }
    let _ = &mut w;
    (dpl, layered, w, h, z)
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
fn sparse_of(d: &TexDesc) -> bool {
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
fn tile_shape_h(d: &TexDesc, dpl: u32, t: Fmt, tile_h: u32) -> u32 {
    if dpl == 2 && d.pkg_layout == 0 && sparse_tile_h(t.bpb) >= 0 {
        sparse_tile_h(t.bpb) as u32
    } else {
        tile_h
    }
}

#[allow(clippy::too_many_arguments)]
fn level_walk(
    d: &TexDesc,
    level: u32,
    dpl: u32,
    w: u32,
    h: u32,
    z: u32,
    tile_h: u32,
    tile_d: u32,
    t: Fmt,
) -> (u64, u32) {
    let (sx, sy) = samples_xy(d.samples);
    let sh = tile_shape_h(d, dpl, t, tile_h);
    let pw = packaged_width_log2(d);
    let mut tw = if pw != 0 {
        1u32 << pw
    } else {
        128u32 >> (sh + tile_d).min(31)
    };
    if tw == 0 {
        tw = 1;
    }
    let mut tiled = sparse_of(d) || pw != 0;
    let mut off = 0u64;
    let mut i = 0u32;
    loop {
        let lw = blk(mip(w * sx, i), t.bw);
        let lh = if dpl >= 2 {
            blk(mip(h * sy, i), t.bh)
        } else {
            1
        };
        let ld = if dpl >= 3 { mip(z, i) } else { 1 };
        let lwb = lw * t.bpb;
        let th = adj_tile(tile_h, 8, lh);
        let td = adj_tile(tile_d, 1, ld);
        let mut wg = lwb.div_ceil(64);
        let hg = lh.div_ceil(8);
        tiled = tiled && lwb >= tw * 64 && lh >= (8u32 << sh) && ld >= (1u32 << tile_d);
        if tiled {
            wg = wg.div_ceil(tw) * tw;
        }
        if i == level {
            return (off, wg);
        }
        let ht = (hg + (1u32 << th) - 1) >> th;
        let dt = (ld + (1u32 << td) - 1) >> td;
        off += ((wg as u64) * ht as u64 * dt as u64) << (9 + th + td);
        i += 1;
    }
}

#[allow(clippy::too_many_arguments)]
fn layer_bytes(
    d: &TexDesc,
    dpl: u32,
    layered: bool,
    w: u32,
    h: u32,
    z: u32,
    tile_h: u32,
    tile_d: u32,
    t: Fmt,
) -> u64 {
    let levels = if d.levels != 0 { d.levels } else { 1 };
    let mut ls = level_walk(d, levels, dpl, w, h, z, tile_h, tile_d, t).0;
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
        let sh = if pw != 0 {
            pw + tile_h + tile_d + 9
        } else {
            th + td + 9
        };
        let g = ls >> sh;
        if ls != g << sh {
            ls = (g + 1) << sh;
        }
    }
    ls
}

/// Storage size, alignment and block shape of a texture (nvnTextureBuilderGetStorageSize and friends).
pub fn tex_layout(d: &TexDesc) -> TexLayout {
    let t = fmt_of(d.format);
    let (dpl, layered, w, h, z) = shape(d);
    let (sx, sy) = samples_xy(d.samples);
    let mut o = TexLayout {
        bpb: t.bpb,
        layers: if layered { z } else { 1 },
        ..Default::default()
    };
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
    if (d.flags & 0x50) != 0 {
        o.align = if (d.flags & 0x40) != 0 { 128 } else { 32 };
        let stride = if d.stride != 0 {
            d.stride
        } else {
            (t.bpb * blk(w, t.bw) + o.align - 1) & !(o.align - 1)
        };
        o.size = stride as u64 * h as u64;
        o.layer_size = o.size;
        return o;
    }
    let (mut tile_h, mut tile_d) = (0u32, 0u32);
    if d.pkg_layout != 0 {
        let pl = ((d.pkg_layout - 1) as u32) & 0xff;
        if dpl >= 2 {
            tile_h = pl & 0xf;
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
        && blk(w * sx, t.bw) * t.bpb
            >= (128u32 >> (sparse_tile_h3(t.bpb) + sparse_tile_d3(t.bpb))) * 64
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
    // MINIMAL_LAYOUT: the automatic block shape only when the layer is no larger than with 1-GOB blocks.
    if (d.flags & 0x200) != 0 && d.pkg_layout == 0 && !sparse_of(d) && (tile_h != 0 || tile_d != 0)
    {
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
    o.align = if sparse_of(d) {
        SPARSE_TILE as u32
    } else {
        512
    };
    o
}

/// Offset of `level` from the start of its layer.
pub fn level_offset(d: &TexDesc, level: u32) -> u64 {
    let t = fmt_of(d.format);
    let (dpl, _, w, h, z) = shape(d);
    if t.bpb == 0 || (d.flags & 0x50) != 0 || d.target == 10 {
        return 0;
    }
    let l = tex_layout(d);
    level_walk(d, level, dpl, w, h, z, l.tile_h, l.tile_d, t).0
}

/// The GOB columns `level` is laid out with.
pub fn level_gobs(d: &TexDesc, level: u32) -> u32 {
    let t = fmt_of(d.format);
    let (dpl, _, w, h, z) = shape(d);
    if t.bpb == 0 || (d.flags & 0x50) != 0 || d.target == 10 {
        return 0;
    }
    let l = tex_layout(d);
    level_walk(d, level, dpl, w, h, z, l.tile_h, l.tile_d, t).1
}

/// Block height / depth (log2 GOBs) that `level` actually uses.
pub fn level_tiles(d: &TexDesc, level: u32) -> (u32, u32) {
    let t = fmt_of(d.format);
    let (dpl, _, _w, h, z) = shape(d);
    let (_, sy) = samples_xy(d.samples);
    let l = tex_layout(d);
    let lh = if dpl >= 2 {
        blk(mip(h * sy, level), t.bh)
    } else {
        1
    };
    let ld = if dpl >= 3 { mip(z, level) } else { 1 };
    (adj_tile(l.tile_h, 8, lh), adj_tile(l.tile_d, 1, ld))
}

/// A sparse texture's tiled levels and one 64 KB tile in texels; 0 for a texture that is not sparse.
pub fn sparse_tiles(d: &TexDesc) -> (i32, u32, u32, u32) {
    let t = fmt_of(d.format);
    let (dpl, _, w, h, z) = shape(d);
    if !sparse_of(d) || t.bpb == 0 {
        return (0, 0, 0, 0);
    }
    let (sx, sy) = samples_xy(d.samples);
    let l = tex_layout(d);
    let sh = tile_shape_h(d, dpl, t, l.tile_h);
    let mut gw = 128u32 >> (sh + l.tile_d);
    if gw == 0 {
        gw = 1;
    }
    let tw = gw * 64 / t.bpb * t.bw / sx;
    let th = if dpl >= 2 {
        (8u32 << sh) * t.bh / sy
    } else {
        1
    };
    let td = if dpl >= 3 { 1u32 << l.tile_d } else { 1 };
    let levels = if d.levels != 0 { d.levels } else { 1 };
    let mut n = 0;
    while n < levels {
        let lwb = blk(mip(w * sx, n), t.bw) * t.bpb;
        let lh = if dpl >= 2 {
            blk(mip(h * sy, n), t.bh)
        } else {
            1
        };
        let ld = if dpl >= 3 { mip(z, n) } else { 1 };
        if lwb < gw * 64 || lh < (8u32 << sh) || ld < (1u32 << l.tile_d) {
            break;
        }
        n += 1;
    }
    (n as i32, tw, th, td)
}

/// The size of a texture's ZCull save area: never 0 (games allocate it from their own pools).
pub fn zcull_size(d: &TexDesc) -> u64 {
    let tiles = d.width.div_ceil(32) as u64
        * d.height.div_ceil(32) as u64
        * if d.depth != 0 { d.depth as u64 } else { 1 };
    if tiles != 0 {
        (tiles * 8 + 0xff) & !0xff
    } else {
        0x100
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Shape {
        what: &'static str,
        target: i32,
        format: i32,
        flags: i32,
        w: u32,
        h: u32,
        depth: u32,
        levels: u32,
        stride: u32,
        pkg: i32,
        size: u64,
        layer: u64,
        off1: u64,
        align: u32,
        th: u32,
        td: u32,
    }
    macro_rules! s {
        ($what:expr, $t:expr, $f:expr, $fl:expr, $w:expr, $h:expr, $d:expr, $l:expr, $st:expr, $pkg:expr, $size:expr, $layer:expr, $off1:expr, $al:expr, $th:expr, $td:expr) => {
            Shape {
                what: $what,
                target: $t,
                format: $f,
                flags: $fl,
                w: $w,
                h: $h,
                depth: $d,
                levels: $l,
                stride: $st,
                pkg: $pkg,
                size: $size,
                layer: $layer,
                off1: $off1,
                align: $al,
                th: $th,
                td: $td,
            }
        };
    }

    /// Shapes the original NVN driver gives (the vectors of nxnative's t_tex_layout.c).
    #[test]
    fn driver_shapes() {
        let driver = [
            s!(
                "packaged ASTC6x6 W4 paper",
                1,
                0x7d,
                4,
                2052,
                1026,
                1,
                1,
                0,
                0x404,
                0x120000,
                0x120000,
                0,
                512,
                3,
                0
            ),
            s!(
                "packaged ASTC6x6 W0 negative control",
                1,
                0x7d,
                4,
                2052,
                1026,
                1,
                1,
                0,
                4,
                0x102000,
                0x102000,
                0,
                512,
                3,
                0
            ),
            s!(
                "packaged ASTC6x6 W4 mip tail",
                1,
                0x7d,
                4,
                2052,
                1026,
                1,
                8,
                0,
                0x404,
                0x19dc00,
                0x19dc00,
                0x120000,
                512,
                3,
                0
            ),
            s!(
                "packaged ASTC6x6 W4 cube tail alignment",
                8,
                0x7d,
                4,
                1024,
                1024,
                6,
                8,
                0,
                0x404,
                0x540000,
                0xe0000,
                0x90000,
                512,
                3,
                0
            ),
            s!(
                "packaged ASTC6x6 W4 array tail alignment",
                4,
                0x7d,
                4,
                2052,
                1026,
                3,
                8,
                0,
                0x404,
                0x4e0000,
                0x1a0000,
                0x120000,
                512,
                3,
                0
            ),
            s!(
                "cube array RGBA8 4x4 depth 6",
                9,
                0x25,
                0,
                4,
                4,
                6,
                1,
                0,
                0,
                3072,
                512,
                0,
                512,
                0,
                0
            ),
            s!(
                "cube array RGBA8 64x64 depth 12, 7 levels",
                9,
                0x25,
                0,
                64,
                64,
                12,
                7,
                0,
                0,
                294912,
                24576,
                16384,
                512,
                3,
                0
            ),
            s!(
                "packaged 3D BC4 64^3, 7 levels, byte 0x40",
                2,
                0x49,
                0,
                64,
                64,
                64,
                7,
                0,
                0x41,
                163328,
                163328,
                131072,
                512,
                0,
                4
            ),
            s!(
                "packaged 3D RGBA8 32^3, byte 0x00",
                2,
                0x25,
                0,
                32,
                32,
                32,
                1,
                0,
                0x01,
                131072,
                131072,
                0,
                512,
                0,
                0
            ),
            s!(
                "packaged 3D RGBA8 32^3, byte 0x12",
                2,
                0x25,
                0,
                32,
                32,
                32,
                1,
                0,
                0x13,
                131072,
                131072,
                0,
                512,
                2,
                1
            ),
            s!(
                "packaged 2D RGBA8 64x64, byte 0x13",
                1,
                0x25,
                0,
                64,
                64,
                1,
                1,
                0,
                0x14,
                16384,
                16384,
                0,
                512,
                3,
                0
            ),
            s!(
                "linear RT RGBA8 260x260 stride 1152",
                1,
                0x25,
                0x40,
                260,
                260,
                1,
                1,
                1152,
                0,
                299520,
                299520,
                0,
                128,
                0,
                0
            ),
            s!(
                "linear RT RGBA8 1x1 stride 128",
                1,
                0x25,
                0x40,
                1,
                1,
                1,
                1,
                128,
                0,
                128,
                128,
                0,
                128,
                0,
                0
            ),
            s!(
                "linear BC1 64x64 stride 128",
                1,
                0x42,
                0x10,
                64,
                64,
                1,
                1,
                128,
                0,
                8192,
                8192,
                0,
                32,
                0,
                0
            ),
            s!(
                "minimal RG8 260x260, 9 levels",
                1,
                0x0d,
                0x200,
                260,
                260,
                1,
                9,
                0,
                0,
                214528,
                214528,
                152064,
                512,
                0,
                0
            ),
            s!(
                "minimal RGB565 320x320 (Depths)",
                1,
                0x3c,
                0x200,
                320,
                320,
                1,
                1,
                0,
                0,
                204800,
                204800,
                0,
                512,
                0,
                0
            ),
            s!(
                "minimal RGB5A1 320x320 (Depths)",
                1,
                0x3b,
                0x200,
                320,
                320,
                1,
                1,
                0,
                0,
                204800,
                204800,
                0,
                512,
                0,
                0
            ),
            s!(
                "minimal RGBA8 64x128, no padding: automatic",
                1,
                0x25,
                0x200,
                64,
                128,
                1,
                1,
                0,
                0,
                32768,
                32768,
                0,
                512,
                4,
                0
            ),
            s!(
                "minimal R8 8x1920, 11 levels",
                1,
                0x01,
                0x200,
                8,
                1920,
                1,
                11,
                0,
                0,
                246784,
                246784,
                122880,
                512,
                0,
                0
            ),
            s!(
                "minimal 3D RGBA8 64x64x8: automatic",
                2,
                0x25,
                0x200,
                64,
                64,
                8,
                1,
                0,
                0,
                131072,
                131072,
                0,
                512,
                0,
                3
            ),
            s!(
                "minimal 3D RGBA8 64x64x9",
                2,
                0x25,
                0x200,
                64,
                64,
                9,
                1,
                0,
                0,
                147456,
                147456,
                0,
                512,
                0,
                0
            ),
            s!(
                "minimal 2D array RGBA8 64x64x3, 7 levels",
                4,
                0x25,
                0x200,
                64,
                64,
                3,
                7,
                0,
                0,
                70656,
                23552,
                16384,
                512,
                0,
                0
            ),
            s!(
                "minimal RGB565 320x320, packaged byte 0x04",
                1,
                0x3c,
                0x200,
                320,
                320,
                1,
                1,
                0,
                5,
                245760,
                245760,
                0,
                512,
                4,
                0
            ),
            s!(
                "sparse DXT5 192x160, under a tile",
                1,
                0x48,
                0x20,
                192,
                160,
                1,
                1,
                0,
                0,
                0x10000,
                0x10000,
                0,
                0x10000,
                2,
                0
            ),
            s!(
                "sparse DXT1 384x384, narrower than a tile",
                1,
                0x45,
                0x20,
                384,
                384,
                1,
                1,
                0,
                0,
                0x20000,
                0x20000,
                0,
                0x10000,
                4,
                0
            ),
            s!(
                "sparse DXT5 384x384, a tile wide",
                1,
                0x48,
                0x20,
                384,
                384,
                1,
                1,
                0,
                0,
                0x40000,
                0x40000,
                0,
                0x10000,
                3,
                0
            ),
            s!(
                "sparse BGRA8 1350x270",
                1,
                0x9c,
                0x20,
                1350,
                270,
                1,
                1,
                0,
                0,
                0x210000,
                0x210000,
                0,
                0x10000,
                4,
                0
            ),
        ];
        let mut fails = vec![];
        for c in &driver {
            let d = TexDesc {
                target: c.target,
                format: c.format,
                flags: c.flags,
                width: c.w,
                height: c.h,
                depth: c.depth,
                levels: c.levels,
                stride: c.stride,
                pkg_layout: c.pkg,
                samples: 0,
            };
            let l = tex_layout(&d);
            let off1 = if c.levels > 1 { level_offset(&d, 1) } else { 0 };
            if l.size != c.size
                || l.layer_size != c.layer
                || off1 != c.off1
                || l.align != c.align
                || l.tile_h != c.th
                || l.tile_d != c.td
            {
                fails.push(format!("{}: got {:?} off1 {off1:#x}", c.what, l));
            }
        }
        let icon = TexDesc {
            target: 1,
            format: 0x48,
            flags: 0x20,
            width: 192,
            height: 160,
            depth: 1,
            levels: 1,
            ..Default::default()
        };
        let (tiled, tw, th, _) = sparse_tiles(&icon);
        if tw != 256 || th != 256 || tiled != 0 {
            fails.push(format!(
                "sparse DXT5 192x160: tile {tw}x{th}, {tiled} tiled levels"
            ));
        }
        assert!(fails.is_empty(), "{fails:#?}");
    }
}
