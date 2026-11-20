//! ASTC decoded on the GPU against ARM's astc-encoder decoding the same blocks on the CPU: images encoded by
//! astcenc at several block footprints (LDR and sRGB), random blocks (every mode the bits pick, error blocks
//! included), and the BC7 re-encode of a large texture.
#![cfg(feature = "gpu-tests")]

mod common;
use ctt_astcenc::{bindings, config_init, Context, Flags, Preset, Profile, Swizzle};

fn image(w: u32, h: u32) -> Vec<u8> {
    let mut v = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let n = (x.wrapping_mul(2654435761) ^ y.wrapping_mul(40503)).rotate_left(7) & 0x3f;
            v.push(((x * 255 / w.max(1)) as u8).wrapping_add((n / 4) as u8));
            v.push((y * 255 / h.max(1)) as u8);
            v.push((((x + y) * 3) & 0xff) as u8 ^ n as u8);
            v.push((255 - (x * 128 / w.max(1))) as u8);
        }
    }
    v
}

fn astc_image(w: u32, h: u32, data: &mut [u8]) -> (bindings::astcenc_image, Box<[*mut core::ffi::c_void; 1]>) {
    let mut slices = Box::new([data.as_mut_ptr() as *mut core::ffi::c_void]);
    let img = bindings::astcenc_image { dim_x: w, dim_y: h, dim_z: 1, data_type: bindings::astcenc_type_ASTCENC_TYPE_U8, data: slices.as_mut_ptr() };
    (img, slices)
}

fn encode(rgba: &[u8], w: u32, h: u32, bw: u32, bh: u32, profile: Profile) -> Vec<u8> {
    let cfg = config_init(profile, bw, bh, 1, Preset::Fast, Flags::USE_DECODE_UNORM8).unwrap();
    let mut ctx = Context::new(&cfg).unwrap();
    let mut src = rgba.to_vec();
    let (mut img, _s) = astc_image(w, h, &mut src);
    let mut out = vec![0u8; (w.div_ceil(bw) * h.div_ceil(bh) * 16) as usize];
    ctx.compress(&mut img, Swizzle::IDENTITY, &mut out).unwrap();
    out
}

fn reference(blocks: &[u8], w: u32, h: u32, bw: u32, bh: u32, profile: Profile) -> Vec<u8> {
    let cfg = config_init(profile, bw, bh, 1, Preset::Fastest, Flags::USE_DECODE_UNORM8 | Flags::DECOMPRESS_ONLY).unwrap();
    let mut ctx = Context::new(&cfg).unwrap();
    let mut out = vec![0u8; (w * h * 4) as usize];
    let (mut img, _s) = astc_image(w, h, &mut out);
    ctx.decompress(blocks, &mut img, Swizzle::IDENTITY).unwrap();
    out
}

/// (texels compared, texels exactly equal, largest channel difference)
fn compare(a: &[u8], b: &[u8]) -> (usize, usize, u8) {
    let mut eq = 0;
    let mut maxd = 0u8;
    for (p, q) in a.chunks(4).zip(b.chunks(4)) {
        let d = (0..4).map(|k| p[k].abs_diff(q[k])).max().unwrap();
        maxd = maxd.max(d);
        eq += (d == 0) as usize;
    }
    (a.len() / 4, eq, maxd)
}

fn gpu() -> &'static nxgfx::vk::Gpu {
    // the back end comes up with a device: initialize one through NVN as a game would
    let mut dev = nxgfx::nvn::types::NVNdevice::boxed();
    let mut db = nxgfx::nvn::types::NVNdeviceBuilder::boxed();
    unsafe {
        nxgfx::nvn::device::nvnDeviceBuilderSetDefaults(&mut *db);
        assert_eq!(nxgfx::nvn::device::nvnDeviceInitialize(&mut *dev, &*db), 1);
    }
    std::mem::forget(dev);
    nxgfx::vk::Gpu::get().unwrap()
}

#[test]
fn astc_gpu_decode_matches_astcenc() {
    let _l = common::setup();
    let e0 = common::validation_errors();
    let g = gpu();
    let (w, h) = (100, 75);
    let src = image(w, h);
    let mut total = (0, 0);
    for (bw, bh) in [(4, 4), (5, 4), (5, 5), (6, 5), (6, 6), (8, 5), (8, 6), (8, 8), (10, 5), (10, 6), (10, 8), (10, 10), (12, 10), (12, 12)] {
        for (profile, srgb) in [(Profile::Ldr, false), (Profile::LdrSrgb, true)] {
            let blocks = encode(&src, w, h, bw, bh, profile);
            let want = reference(&blocks, w, h, bw, bh, profile);
            let got = nxgfx::tex::astc::decode_blocks(g, &blocks, w, h, bw, bh, srgb, false).expect("GPU decode");
            let (n, eq, maxd) = compare(&got, &want);
            eprintln!("ASTC {bw}x{bh}{}: {n} texels, {eq} identical, largest difference {maxd}", if srgb { " sRGB" } else { "" });
            assert!(maxd <= 1, "ASTC {bw}x{bh} srgb={srgb}: difference {maxd}");
            total.0 += n;
            total.1 += eq;
        }
    }
    eprintln!("ASTC encoded images: {} of {} texels identical to astcenc", total.1, total.0);
    assert_eq!(common::validation_errors(), e0, "validation errors");
}

#[test]
fn astc_random_blocks_match_astcenc() {
    let _l = common::setup();
    let e0 = common::validation_errors();
    let g = gpu();
    // xorshift: every bit pattern a block can have, valid or not
    let mut s = 0x9e3779b97f4a7c15u64;
    let mut rnd = || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s
    };
    let mut worst = 0u8;
    let (mut n_all, mut eq_all) = (0, 0);
    for (bw, bh) in [(4, 4), (6, 6), (8, 8), (12, 12)] {
        let (bx, by) = (32u32, 32u32);
        let (w, h) = (bx * bw, by * bh);
        let blocks: Vec<u8> = (0..bx * by * 2).flat_map(|_| rnd().to_le_bytes()).collect();
        let want = reference(&blocks, w, h, bw, bh, Profile::Ldr);
        let got = nxgfx::tex::astc::decode_blocks(g, &blocks, w, h, bw, bh, false, false).expect("GPU decode");
        let (n, eq, maxd) = compare(&got, &want);
        eprintln!("ASTC {bw}x{bh} random blocks: {n} texels, {eq} identical, largest difference {maxd}");
        worst = worst.max(maxd);
        n_all += n;
        eq_all += eq;
    }
    eprintln!("ASTC random blocks: {eq_all} of {n_all} texels identical to astcenc");
    assert!(worst <= 1, "random blocks: largest difference {worst}");
    assert_eq!(common::validation_errors(), e0, "validation errors");
}

const P2: [u16; 64] = [
    0xcccc, 0x8888, 0xeeee, 0xecc8, 0xc880, 0xfeec, 0xfec8, 0xec80, 0xc800, 0xffec, 0xfe80, 0xe800, 0xffe8, 0xff00, 0xfff0, 0xf000, 0xf710, 0x008e, 0x7100, 0x08ce, 0x008c,
    0x7310, 0x3100, 0x8cce, 0x088c, 0x3110, 0x6666, 0x366c, 0x17e8, 0x0ff0, 0x718e, 0x399c, 0xaaaa, 0xf0f0, 0x5a5a, 0x33cc, 0x3c3c, 0x55aa, 0x9696, 0xa55a, 0x73ce, 0x13c8,
    0x324c, 0x3bdc, 0x6996, 0xc33c, 0x9966, 0x0660, 0x0272, 0x04e4, 0x4e40, 0x2720, 0xc936, 0x936c, 0x39c6, 0x639c, 0x9336, 0x9cc6, 0x817e, 0xe718, 0xccf0, 0x0fcc, 0x7744,
    0xee22,
];
const A2: [u32; 64] = [
    15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 2, 8, 2, 2, 8, 8, 15, 2, 8, 2, 2, 8, 8, 2, 2, 15, 15, 6, 8, 2, 8, 15, 15, 2, 8, 2, 2, 2, 15, 15, 6, 6, 2, 6,
    8, 15, 15, 2, 2, 15, 15, 15, 15, 15, 2, 2, 15,
];

/// BC7 mode 5: RGB 7.7.7 and alpha 8.8, 2-bit indices each, a rotation.
fn bc7_mode5(v: u128) -> [[u8; 4]; 16] {
    let mut at = 6;
    let mut rd = |n: u32| {
        let r = ((v >> at) & ((1u128 << n) - 1)) as u32;
        at += n;
        r
    };
    let rot = rd(2);
    let mut e = [[0u32; 4]; 2];
    for ch in 0..3 {
        for ep in e.iter_mut() {
            let x = rd(7);
            ep[ch] = (x << 1) | (x >> 6);
        }
    }
    e[0][3] = rd(8);
    e[1][3] = rd(8);
    let ci: Vec<u32> = (0..16).map(|i| rd(if i == 0 { 1 } else { 2 })).collect();
    let ai: Vec<u32> = (0..16).map(|i| rd(if i == 0 { 1 } else { 2 })).collect();
    let w2 = [0u32, 21, 43, 64];
    let mut out = [[0u8; 4]; 16];
    for i in 0..16 {
        for ch in 0..4 {
            let k = if ch < 3 { ci[i] } else { ai[i] };
            out[i][ch] = (((64 - w2[k as usize]) * e[0][ch] + w2[k as usize] * e[1][ch] + 32) >> 6) as u8;
        }
        if rot > 0 {
            out[i].swap(3, rot as usize - 1);
        }
    }
    out
}

/// BC7 decode of the modes the encoder writes (1, 3, 5, 6, 7), from the Khronos layout: (mode, texels). Mode 8 =
/// other.
fn bc7_decode(blk: &[u8; 16]) -> (u32, [[u8; 4]; 16]) {
    let v = u128::from_le_bytes(*blk);
    let mut out = [[0u8; 4]; 16];
    let mode = v.trailing_zeros();
    if mode == 5 {
        return (5, bc7_mode5(v));
    }
    // (subsets, colour bits, alpha bits, p-bits: 1 = shared per subset, 2 = one per endpoint, index bits)
    let (ns, cb, ab, pt, ib) = match mode {
        1 => (2u32, 6u32, 0u32, 1u32, 3u32),
        3 => (2, 7, 0, 2, 2),
        6 => (1, 7, 7, 2, 4),
        7 => (2, 5, 5, 2, 2),
        _ => return (8, out),
    };
    let mut at = mode + 1;
    let mut rd = |n: u32| {
        let r = ((v >> at) & ((1u128 << n) - 1)) as u32;
        at += n;
        r
    };
    let part = if ns == 2 { rd(6) } else { 0 };
    let mut e = [[0u32; 4]; 4];
    for ch in 0..4 {
        let bits = if ch < 3 { cb } else { ab };
        for ep in e.iter_mut().take(2 * ns as usize) {
            ep[ch] = if bits > 0 { rd(bits) } else { 255 };
        }
    }
    let mut p = [0u32; 4];
    if pt == 1 {
        let (a, b) = (rd(1), rd(1));
        p = [a, a, b, b];
    } else {
        for x in p.iter_mut().take(2 * ns as usize) {
            *x = rd(1);
        }
    }
    let mask = if ns == 2 { P2[part as usize] } else { 0 };
    let anchor1 = if ns == 2 { A2[part as usize] } else { 99 };
    let w = |k: u32| -> u32 {
        match ib {
            2 => [0, 21, 43, 64][k as usize],
            3 => [0, 9, 18, 27, 37, 46, 55, 64][k as usize],
            _ => [0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64][k as usize],
        }
    };
    let exp = |x: u32, p: u32, bits: u32| -> u32 {
        let n = bits + 1;
        let f = (x << 1) | p;
        if n >= 8 {
            f
        } else {
            (f << (8 - n)) | (f >> (2 * n - 8))
        }
    };
    for (i, t) in out.iter_mut().enumerate() {
        let i = i as u32;
        let s = ((mask >> i) & 1) as usize;
        let n = if i == 0 || i == anchor1 { ib - 1 } else { ib };
        let k = rd(n);
        for ch in 0..4 {
            let bits = if ch < 3 { cb } else { ab };
            let (a, b) = if bits == 0 { (255, 255) } else { (exp(e[2 * s][ch], p[2 * s], bits), exp(e[2 * s + 1][ch], p[2 * s + 1], bits)) };
            t[ch] = (((64 - w(k)) * a + w(k) * b + 32) >> 6) as u8;
        }
    }
    (mode, out)
}

/// (PSNR against `rgba`, blocks per mode 0..8, the decoded texels)
fn bc7_score(bc7: &[u8], rgba: &[u8], w: u32, h: u32) -> (f64, [u32; 9], Vec<u8>) {
    let mut se = 0f64;
    let mut modes = [0u32; 9];
    let mut dec = vec![0u8; (w * h * 4) as usize];
    for by in 0..h / 4 {
        for bx in 0..w / 4 {
            let k = ((by * (w / 4) + bx) * 16) as usize;
            let (m, t) = bc7_decode(bc7[k..k + 16].try_into().unwrap());
            modes[m as usize] += 1;
            for i in 0..16 {
                let (x, y) = (bx * 4 + i % 4, by * 4 + i / 4);
                let o = ((y * w + x) * 4) as usize;
                for c in 0..4 {
                    dec[o + c] = t[i as usize][c];
                    let d = t[i as usize][c] as f64 - rgba[o + c] as f64;
                    se += d * d;
                }
            }
        }
    }
    let mse = se / (w * h * 4) as f64;
    (10.0 * (255.0f64 * 255.0 / mse.max(1e-9)).log10(), modes, dec)
}

/// Edges, stripes, a smooth gradient meeting flat colour, and transparent cut-outs: what one line per block (mode 6)
/// cannot hold.
fn hard_image(w: u32, h: u32, alpha: bool) -> Vec<u8> {
    let mut v = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let (r, g, b) = if (x / 3 + y / 5) % 2 == 0 { (230, 40, 30) } else { (20, 90 + (y % 64) as u8, 210) };
            let (r, g, b) = if x > w / 2 { ((x * 255 / w) as u8, (y * 255 / h) as u8, if (x + 2 * y) % 7 < 3 { 255 } else { 0 }) } else { (r, g, b) };
            let a = if alpha && (x * x + y) % 13 < 4 { 0 } else { 255 };
            v.extend_from_slice(&[r, g, b, a]);
        }
    }
    v
}

/// `rgba` through ASTC 4x4 (the thorough preset) and the GPU's ASTC decode and BC7 encode: (the BC7 blocks, the
/// RGBA8 decode the encoder started from).
fn encode_bc7(g: &'static nxgfx::vk::Gpu, rgba: &[u8], w: u32, h: u32, mode6: bool) -> (Vec<u8>, Vec<u8>) {
    let cfg = config_init(Profile::Ldr, 4, 4, 1, Preset::Thorough, Flags::USE_DECODE_UNORM8).unwrap();
    let mut ctx = Context::new(&cfg).unwrap();
    let mut src = rgba.to_vec();
    let (mut img, _s) = astc_image(w, h, &mut src);
    let mut blocks = vec![0u8; (w.div_ceil(4) * h.div_ceil(4) * 16) as usize];
    ctx.compress(&mut img, Swizzle::IDENTITY, &mut blocks).unwrap();
    let texels = nxgfx::tex::astc::decode_blocks(g, &blocks, w, h, 4, 4, false, false).unwrap();
    assert!(nxgfx::tex::astc::wait_bc7_encoder(g), "the full BC7 encoder's pipeline");
    nxgfx::tex::astc::BC7_MODE6_ONLY.store(mode6, std::sync::atomic::Ordering::Relaxed);
    let out = nxgfx::tex::astc::decode_blocks(g, &blocks, w, h, 4, 4, false, true).unwrap();
    nxgfx::tex::astc::BC7_MODE6_ONLY.store(false, std::sync::atomic::Ordering::Relaxed);
    (out, texels)
}

#[test]
fn astc_to_bc7_quality() {
    let _l = common::setup();
    let e0 = common::validation_errors();
    let g = gpu();
    let (w, h) = (256, 256);
    let src = image(w, h);
    let blocks = encode(&src, w, h, 6, 6, Profile::Ldr);
    let rgba = nxgfx::tex::astc::decode_blocks(g, &blocks, w, h, 6, 6, false, false).unwrap();
    assert!(nxgfx::tex::astc::wait_bc7_encoder(g), "the full BC7 encoder's pipeline");
    let bc7 = nxgfx::tex::astc::decode_blocks(g, &blocks, w, h, 6, 6, false, true).unwrap();
    let (psnr, modes, _) = bc7_score(&bc7, &rgba, w, h);
    eprintln!("ASTC 6x6 -> BC7 (256x256): {psnr:.1} dB against the RGBA8 decode, blocks per mode {modes:?}");
    assert!(psnr > 30.0, "BC7 re-encode PSNR {psnr:.1} dB");
    assert_eq!(modes[8], 0, "only modes 1, 3, 5, 6 and 7");
    assert_eq!(common::validation_errors(), e0, "validation errors");
}

/// The two-subset modes (1 and 3 opaque, 7 with alpha) and mode 5 (alpha apart) where mode 6 cannot hold a block: higher PSNR than mode 6
/// alone, alpha 0 kept, and every block read by the GPU's own BC7 decoder as the CPU decode here reads it (the bit
/// layouts).
#[test]
fn bc7_modes_beyond_6_beat_mode_6_alone() {
    let _l = common::setup();
    let e0 = common::validation_errors();
    let g = gpu();
    let (w, h) = (256, 256);
    for alpha in [false, true] {
        let src = hard_image(w, h, alpha);
        let (six, rgba) = encode_bc7(g, &src, w, h, true);
        let (all, _) = encode_bc7(g, &src, w, h, false);
        let (p6, m6, _) = bc7_score(&six, &rgba, w, h);
        let (pa, ma, dec) = bc7_score(&all, &rgba, w, h);
        eprintln!("BC7 {}: mode 6 alone {p6:.2} dB {m6:?}; all modes {pa:.2} dB {ma:?}", if alpha { "with alpha" } else { "opaque" });
        assert_eq!(ma[8], 0, "only modes 1, 3, 5, 6 and 7");
        assert!(pa > p6 + 1.0, "all modes {pa:.2} dB against mode 6 {p6:.2} dB");
        if alpha {
            assert!(ma[5] > 0 && ma[7] > 0, "modes 5 and 7 used");
        } else {
            assert!(ma[1] + ma[3] > 0, "modes 1 / 3 used");
        }
        // alpha 0 kept where it was (UI quads): no more texels lose it than with mode 6 alone
        let lost = |d: &[u8]| rgba.chunks(4).zip(d.chunks(4)).filter(|(s, d)| s[3] == 0 && d[3] != 0).count();
        let (l6, la) = (lost(&bc7_score(&six, &rgba, w, h).2), lost(&dec));
        eprintln!("  alpha-0 texels that lost it: mode 6 alone {l6}, all modes {la}");
        assert!(la <= l6, "alpha 0 kept as well as mode 6 keeps it");
        match nxgfx::tex::astc::bc7_hw_decode(g, &all, w, h) {
            Some(hw) => assert!(hw == dec, "the GPU's BC7 decoder reads the blocks as the layout here does"),
            None => eprintln!("no BC7 blit on this device: hardware check skipped"),
        }
    }
    assert_eq!(common::validation_errors(), e0, "validation errors");
}

/// The disk cache: a level encoded once is uploaded from the pack the next time, byte for byte, and after a reopen.
#[test]
fn bc7_levels_come_back_from_the_disk_cache() {
    let _l = common::setup();
    let e0 = common::validation_errors();
    let g = gpu();
    let dir = std::env::temp_dir().join(format!("nxgfx-astc-gpu-cache-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    nxgfx::tex::astc_cache::set_dir(&dir);
    let (w, h) = (256, 256);
    let blocks = encode(&image(w, h), w, h, 6, 6, Profile::Ldr);
    assert!(nxgfx::tex::astc::wait_bc7_encoder(g), "the full BC7 encoder's pipeline");
    let (h0, _, p0) = nxgfx::tex::astc_cache::stats();
    let first = nxgfx::tex::astc::decode_blocks(g, &blocks, w, h, 6, 6, false, true).unwrap();
    nxgfx::tex::astc_cache::flush();
    let (h1, _, p1) = nxgfx::tex::astc_cache::stats();
    assert_eq!((h1, p1), (h0, p0 + 1), "a miss, then written");
    let second = nxgfx::tex::astc::decode_blocks(g, &blocks, w, h, 6, 6, false, true).unwrap();
    assert_eq!(nxgfx::tex::astc_cache::stats().0, h1 + 1, "a hit");
    assert!(first == second);
    nxgfx::tex::astc_cache::close();
    nxgfx::tex::astc_cache::set_dir(&dir);
    let third = nxgfx::tex::astc::decode_blocks(g, &blocks, w, h, 6, 6, false, true).unwrap();
    assert_eq!(nxgfx::tex::astc_cache::stats().0, h1 + 2, "a hit after a reopen");
    assert!(first == third);
    nxgfx::tex::astc_cache::close();
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(common::validation_errors(), e0, "validation errors");
}

#[test]
#[ignore]
fn bc7_encode_timing() {
    let _l = common::setup();
    let g = gpu();
    let (w, h) = (4096, 4096);
    let src = hard_image(w, h, true);
    let cfg = config_init(Profile::Ldr, 8, 8, 1, Preset::Fastest, Flags::USE_DECODE_UNORM8).unwrap();
    let mut ctx = Context::new(&cfg).unwrap();
    let mut s2 = src.clone();
    let (mut img, _s) = astc_image(w, h, &mut s2);
    let mut blocks = vec![0u8; (w.div_ceil(8) * h.div_ceil(8) * 16) as usize];
    ctx.compress(&mut img, Swizzle::IDENTITY, &mut blocks).unwrap();
    let t = std::time::Instant::now();
    nxgfx::tex::astc::wait_bc7_encoder(g);
    eprintln!("pipelines: {:?}", t.elapsed());
    for m6 in [true, false, true, false] {
        nxgfx::tex::astc::BC7_MODE6_ONLY.store(m6, std::sync::atomic::Ordering::Relaxed);
        let t = std::time::Instant::now();
        nxgfx::tex::astc::decode_blocks(g, &blocks, w, h, 8, 8, false, true).unwrap();
        eprintln!("4096x4096 mode6={m6}: {:?}", t.elapsed());
    }
}
