//! CPU oracle for synthetic, openly generated weights. Kept out of production builds.
//! Arithmetic target: retained d15370a382630d4e.spv, SHA-256
//! 8c72084e7495f8429a7ca8a31544e7967cb26ad1b81072c3bd8807b144519631.
//! DS-01 symbolically traces all 128 dmsh_hmma16816_f16 output chains: unpack C,
//! eight packed A/B words, low-half Fma then high-half Fma, one final half pack.
//! No claim about console HMMA or Apple's intra-matrix reduction order.
use super::{kernels::Job, Kernel};

pub fn f(h: u16) -> f32 {
    crate::vk::shot::half(h)
}
pub fn h(x: f32) -> u16 {
    let bits = x.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let e = ((bits >> 23) & 255) as i32;
    let m = bits & 0x7fffff;
    if e == 255 {
        return sign | if m == 0 { 0x7c00 } else { 0x7e00 };
    }
    if e > 142 {
        return sign | 0x7c00;
    }
    if e < 102 {
        return sign;
    }
    let (v, shift) = if e < 113 {
        (m | 0x800000, (126 - e) as u32)
    } else {
        (m | (((e - 112) as u32) << 23), 13)
    };
    let base = v >> shift;
    let rem = v & ((1 << shift) - 1);
    let halfway = 1 << (shift - 1);
    sign | (base + u32::from(rem > halfway || rem == halfway && base & 1 != 0)) as u16
}
pub fn round(x: f32) -> f32 {
    f(h(x))
}
pub fn put(v: &mut [u32], i: usize, x: f32) {
    let shift = 16 * (i & 1);
    v[i / 2] = (v[i / 2] & !(0xffff << shift)) | (u32::from(h(x)) << shift);
}
pub fn get(v: &[u32], i: usize) -> f32 {
    f((v[i / 2] >> (16 * (i & 1))) as u16)
}
pub fn addr(x: usize, y: usize, w: usize, c: usize, n: usize) -> usize {
    ((y + 1) * (w + 2) + x + 1) * c + n
}
pub fn rgb8(v: u32) -> [f32; 4] {
    std::array::from_fn(|i| ((v >> (8 * i)) & 255) as f32 / 255.0)
}
pub fn rgb10(v: u32) -> [f32; 4] {
    [
        (v & 1023) as f32 / 1023.0,
        ((v >> 10) & 1023) as f32 / 1023.0,
        ((v >> 20) & 1023) as f32 / 1023.0,
        (v >> 30) as f32 / 3.0,
    ]
}
fn unorm(x: f32, max: u32) -> u32 {
    (x.clamp(0.0, 1.0) * max as f32).round_ties_even() as u32
}
fn pack10(v: [f32; 4]) -> u32 {
    unorm(v[0], 1023)
        | (unorm(v[1], 1023) << 10)
        | (unorm(v[2], 1023) << 20)
        | (unorm(v[3], 3) << 30)
}

/// One scalar output of the retained lane/fragment helper, expressed in its packed
/// word order rather than the native shader's channel/modulo loop. Half conversion
/// uses round-to-nearest-even, preserving subnormals; device parity remains a gate.
pub fn hmma16(a: [u32; 8], b: [u32; 8], c: u16) -> u16 {
    packed_dot(a, b, c, false)
}

fn packed_dot(a: [u32; 8], b: [u32; 8], c: u16, eight: bool) -> u16 {
    let mut acc = f(c);
    for (word, (a, b)) in a.into_iter().zip(b).enumerate() {
        acc = f(a as u16).mul_add(f(b as u16), acc);
        acc = f((a >> 16) as u16).mul_add(f((b >> 16) as u16), acc);
        // Comparison only: reproduces the old scalar eight-product prototype,
        // not an emulation of Apple's matrix multiply implementation.
        if eight && word == 3 {
            acc = round(acc);
        }
    }
    h(acc)
}

fn dot(c: f32, operands: impl Fn(usize) -> (f32, f32), eight: bool) -> f32 {
    let mut a = [0; 8];
    let mut b = [0; 8];
    for i in 0..8 {
        let (al, bl) = operands(2 * i);
        let (ah, bh) = operands(2 * i + 1);
        a[i] = u32::from(h(al)) | (u32::from(h(ah)) << 16);
        b[i] = u32::from(h(bl)) | (u32::from(h(bh)) << 16);
    }
    f(if eight {
        packed_dot(a, b, h(c), true)
    } else {
        hmma16(a, b, h(c))
    })
}

pub fn run(job: Job, buffers: [&[u32]; 5]) -> Vec<u32> {
    run_contract(job, buffers, false)
}

pub fn run_eight_product_comparison(job: Job, buffers: [&[u32]; 5]) -> Vec<u32> {
    run_contract(job, buffers, true)
}

fn run_contract(job: Job, buffers: [&[u32]; 5], eight: bool) -> Vec<u32> {
    let [input, _, weights, bias, skip] = buffers;
    let (w, hh) = (job.width as usize, job.height as usize);
    let [_, ki, no] = job.kernel.shape();
    let (ki, no) = (ki as usize, no as usize);
    let mut out = vec![0; (job.extents().unwrap()[1] / 4) as usize];
    match job.kernel {
        Kernel::Pack => {
            for y in 0..hh {
                for x in 0..w {
                    for sub in 0..4 {
                        let pixel = rgb8(input[(2 * y + sub / 2) * (2 * w) + 2 * x + sub % 2]);
                        for (c, value) in pixel.into_iter().enumerate() {
                            put(&mut out, addr(x, y, w, 16, 4 * sub + c), value);
                        }
                    }
                }
            }
        }
        Kernel::Luma => {
            for i in 0..w * hh {
                let c = rgb10(input[i]);
                put(
                    &mut out,
                    i,
                    c[2].mul_add(0.0722, c[0].mul_add(0.2126, c[1] * 0.7152)),
                );
            }
        }
        Kernel::Compare => {
            out[1] = u32::MAX;
            for i in 0..w * hh {
                if input[i] != skip[i] {
                    out[0] += 1;
                    out[1] = out[1].min(i as u32);
                }
            }
        }
        Kernel::Resample => resample(job, input, weights, bias, skip, &mut out, eight),
        kernel => {
            let up = matches!(kernel, Kernel::Up64 | Kernel::Up32);
            let down = matches!(kernel, Kernel::Down16 | Kernel::Down32);
            let (ow, oh) = if up { (2 * w, 2 * hh) } else { (w, hh) };
            let iw = if down { 2 * w } else { w };
            for y in 0..oh {
                for x in 0..ow {
                    let mut channels = vec![0.0; no];
                    for (n, d) in channels.iter_mut().enumerate() {
                        let mut acc = if up {
                            get(skip, addr(x, y, ow, no, n))
                        } else {
                            0.0
                        };
                        let taps = if up {
                            1
                        } else if down {
                            4
                        } else {
                            9
                        };
                        for t in 0..taps {
                            let wt = if up { 2 * (y % 2) + x % 2 } else { t };
                            let (ix, iy) = if up {
                                (x / 2 + 1, y / 2 + 1)
                            } else if down {
                                (2 * x + t % 2 + 1, 2 * y + t / 2 + 1)
                            } else {
                                (x + t % 3, y + t / 3)
                            };
                            for kb in (0..ki).step_by(16) {
                                acc = dot(
                                    acc,
                                    |j| {
                                        (
                                            get(input, (iy * (iw + 2) + ix) * ki + kb + j),
                                            get(weights, (wt * ki + kb + j) * no + n),
                                        )
                                    },
                                    eight,
                                );
                            }
                        }
                        *d = acc;
                        if kernel != Kernel::Out {
                            put(
                                &mut out,
                                addr(x, y, ow, no, n),
                                (acc + get(bias, n)).max(0.0),
                            );
                        }
                    }
                    if kernel == Kernel::Out {
                        for sub in 0..4 {
                            let at = (2 * y + sub / 2) * 2 * w + 2 * x + sub % 2;
                            let pic = rgb8(skip[at]);
                            let ns = [2 * sub, 2 * sub + 1, 8 + 2 * sub, 9 + 2 * sub];
                            out[at] = pack10(std::array::from_fn(|c| {
                                (pic[c] + channels[ns[c]]) + get(bias, ns[c])
                            }));
                        }
                    }
                }
            }
        }
    }
    out
}

fn resample(
    job: Job,
    luma: &[u32],
    weights: &[u32],
    dither: &[u32],
    pic: &[u32],
    out: &mut [u32],
    eight: bool,
) {
    let (w, h) = (job.width as usize, job.height as usize);
    let (ow, oh) = (w * 4 / 3, h * 4 / 3);
    for by in 0..oh / 8 {
        for bx in 0..ow / 8 {
            let mut x = [[0.0; 16]; 16];
            for r in 0..16 {
                for c in 0..16 {
                    let py = (6 * by as isize - 5 + r as isize).clamp(0, h as isize - 1) as usize;
                    let px = (6 * bx as isize - 5 + c as isize).clamp(0, w as isize - 1) as usize;
                    x[r][c] = get(luma, py * w + px);
                }
            }
            for block in 0..6 {
                let mut hidden = [[[0.0; 16]; 16]; 2];
                for g in 0..2 {
                    for m in 0..16 {
                        for n in 0..16 {
                            hidden[g][m][n] = dot(
                                0.0,
                                |k| {
                                    (
                                        get(
                                            weights,
                                            block * 1024 + 256 * g + 128 * (k / 8) + 8 * m + k % 8,
                                        ),
                                        x[k][n],
                                    )
                                },
                                eight,
                            );
                        }
                    }
                }
                for m in 0..16 {
                    for n in 0..16 {
                        let mut a = 0.0;
                        for g in 0..2 {
                            a = dot(
                                a,
                                |k| {
                                    (
                                        hidden[g][m][k],
                                        get(
                                            weights,
                                            block * 1024
                                                + 512
                                                + 256 * g
                                                + 128 * (n / 8)
                                                + 8 * k
                                                + n % 8,
                                        ),
                                    )
                                },
                                eight,
                            );
                        }
                        x[m][n] = if block < 5 { a.max(0.0) } else { a };
                    }
                }
            }
            let fetch = |px: isize, py: isize| {
                rgb10(
                    pic[py.clamp(0, h as isize - 1) as usize * w
                        + px.clamp(0, w as isize - 1) as usize],
                )
            };
            for r in 0..8 {
                for c in 0..8 {
                    let (px, py) = (8 * bx + c, 8 * by + r);
                    let qx = ((px as f32 + 0.5) * (1.0 / ow as f32)) * w as f32 - 0.5;
                    let qy = ((py as f32 + 0.5) * (1.0 / oh as f32)) * h as f32 - 0.5;
                    let (ix, iy) = (qx.floor() as isize, qy.floor() as isize);
                    let (fx, fy) = (qx - qx.floor(), qy - qy.floor());
                    let [a, b, c0, d] = [
                        fetch(ix, iy),
                        fetch(ix + 1, iy),
                        fetch(ix, iy + 1),
                        fetch(ix + 1, iy + 1),
                    ];
                    let dv = rgb10(
                        dither[(py % job.dither[1] as usize) * job.dither[0] as usize
                            + px % job.dither[0] as usize],
                    );
                    let mut packed = 0xff000000;
                    for channel in 0..3 {
                        let top = a[channel] * (1.0 - fx) + b[channel] * fx;
                        let bottom = c0[channel] * (1.0 - fx) + d[channel] * fx;
                        let sample = top * (1.0 - fy) + bottom * fy;
                        let v = -0.0 + (sample + x[r][c]);
                        let k = v
                            .mul_add(255.0, dv[channel].mul_add(2.0, -1.0))
                            .round_ties_even();
                        packed |= unorm(k * 0.003921569, 255) << (8 * channel);
                    }
                    let yo = if job.flip { oh - 1 - py } else { py };
                    out[yo * ow + px] = packed;
                }
            }
        }
    }
}
