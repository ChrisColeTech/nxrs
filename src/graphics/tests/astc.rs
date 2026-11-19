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

/// BC7 mode 6 (the only mode the encoder writes): 7-bit RGBA endpoints with a p-bit each, 4-bit indices.
fn bc7_mode6_decode(blk: &[u8; 16]) -> [[u8; 4]; 16] {
    let v = u128::from_le_bytes(*blk);
    let mut out = [[0u8; 4]; 16];
    if v & 0x7f != 0x40 {
        return out;
    }
    let bits = |o: u32, n: u32| ((v >> o) & ((1u128 << n) - 1)) as u32;
    let mut e = [[0u32; 4]; 2];
    for c in 0..4 {
        e[0][c] = bits(7 + 14 * c as u32, 7);
        e[1][c] = bits(14 + 14 * c as u32, 7);
    }
    let p = [bits(63, 1), bits(64, 1)];
    let w4 = [0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64];
    for i in 0..16 {
        let idx = if i == 0 { bits(65, 3) } else { bits(68 + 4 * (i as u32 - 1), 4) };
        for c in 0..4 {
            let a = (e[0][c] << 1) | p[0];
            let b = (e[1][c] << 1) | p[1];
            let wv = w4[idx as usize];
            out[i][c] = (((64 - wv) * a + wv * b + 32) >> 6) as u8;
        }
    }
    out
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
    let bc7 = nxgfx::tex::astc::decode_blocks(g, &blocks, w, h, 6, 6, false, true).unwrap();
    let mut se = 0f64;
    for by in 0..h / 4 {
        for bx in 0..w / 4 {
            let k = ((by * (w / 4) + bx) * 16) as usize;
            let t = bc7_mode6_decode(bc7[k..k + 16].try_into().unwrap());
            for i in 0..16 {
                let (x, y) = (bx * 4 + i % 4, by * 4 + i / 4);
                let o = ((y * w + x) * 4) as usize;
                for c in 0..4 {
                    let d = t[i as usize][c] as f64 - rgba[o + c] as f64;
                    se += d * d;
                }
            }
        }
    }
    let mse = se / (w * h * 4) as f64;
    let psnr = 10.0 * (255.0f64 * 255.0 / mse.max(1e-9)).log10();
    eprintln!("ASTC 6x6 -> BC7 (256x256): {psnr:.1} dB against the RGBA8 decode");
    assert!(psnr > 30.0, "BC7 re-encode PSNR {psnr:.1} dB");
    assert_eq!(common::validation_errors(), e0, "validation errors");
}
