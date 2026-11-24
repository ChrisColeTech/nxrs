//! Expected bits below are dyadic counterexamples, independent of the native GLSL.
//! Helper provenance/order: DS-01 helper-contract.json, retained SPIR-V SHA-256
//! 8c72084e7495f8429a7ca8a31544e7967cb26ad1b81072c3bd8807b144519631.
use super::{kernels::Job, reference as r, Kernel};

fn words(halves: [u16; 16]) -> [u32; 8] {
    std::array::from_fn(|i| u32::from(halves[2 * i]) | (u32::from(halves[2 * i + 1]) << 16))
}

fn dot(c: u16, terms: &[(usize, u16, u16)]) -> u16 {
    let mut a = [0; 16];
    let mut b = [0x3c00; 16];
    for &(k, av, bv) in terms {
        a[k] = av;
        b[k] = bv;
    }
    r::hmma16(words(a), words(b), c)
}

#[test]
fn helper_midpoint_ties_signs_and_subnormal_products() {
    // 1 + 2^-11 + 2^-11: the k=7/8 boundary must NOT round the first tie.
    let positive = [(7, 0x1000, 0x3c00), (8, 0x1000, 0x3c00)];
    let negative = [(7, 0x9000, 0x3c00), (8, 0x9000, 0x3c00)];
    assert_eq!(dot(0x3c00, &positive), 0x3c01);
    assert_eq!(dot(0xbc00, &negative), 0xbc01);
    // Final ties go to an even half mantissa, for both signs.
    for (c, term, want) in [
        (0x3c00, 0x1000, 0x3c00),
        (0x3c01, 0x1000, 0x3c02),
        (0xbc00, 0x9000, 0xbc00),
        (0xbc01, 0x9000, 0xbc02),
    ] {
        assert_eq!(dot(c, &[(15, term, 0x3c00)]), want);
    }
    // Each product is below the minimum half subnormal, but their sum is not.
    for sign in [0, 0x8000] {
        let terms = [(7, sign | 1, 0x3800), (8, sign | 1, 0x3800)];
        assert_eq!(dot(0, &terms), sign | 1);
        assert_eq!(dot(0, &[(15, sign | 1, 0x3800)]), sign);
        assert_eq!(dot(sign | 1, &[(15, sign | 1, 0x3800)]), sign | 2);
    }
    // Largest subnormal plus the minimum subnormal reaches the smallest normal.
    assert_eq!(dot(0x03ff, &[(15, 1, 0x3c00)]), 0x0400);
    // A chain of -0 products added to -0 retains its sign before any ReLU.
    let negative_zero = r::hmma16(words([0x8000; 16]), words([0x3c00; 16]), 0x8000);
    assert_eq!(negative_zero, 0x8000);
}

#[test]
fn helper_preserves_fp32_low_then_high_order_and_only_final_half_overflow() {
    // 4096*4096 + 1 - 4096*4096 = 0 with the retained FP32 left fold.
    // A wider accumulator, or a reassociation that cancels first, produces 1.
    let lost_unit = [
        (0, 0x6c00, 0x6c00),
        (1, 0x3c00, 0x3c00),
        (2, 0xec00, 0x6c00),
    ];
    let kept_unit = [
        (0, 0x6c00, 0x6c00),
        (1, 0xec00, 0x6c00),
        (2, 0x3c00, 0x3c00),
    ];
    assert_eq!(dot(0, &lost_unit), 0);
    assert_eq!(dot(0, &kept_unit), 0x3c00);
    // The midpoint 65520 must not overflow to half at k=7 before cancellation.
    let cancelled = [(7, 0x4c00, 0x3c00), (8, 0xcc00, 0x3c00)];
    assert_eq!(dot(0x7bff, &cancelled), 0x7bff);
    assert_eq!(dot(0x7bff, &[(15, 0x4c00, 0x3c00)]), 0x7c00);
    assert_eq!(dot(0xfbff, &[(15, 0xcc00, 0x3c00)]), 0xfc00);
}

/// Also consumed by the ignored GPU gate: expected bits do not come from run().
pub(super) fn boundary_witness(pair: [usize; 2]) -> (Job, [Vec<u32>; 5]) {
    let (job, mut data) = super::tests::rounding_witness();
    data[2].fill(0);
    for t in 0..4 {
        for n in 0..32 {
            for k in pair {
                r::put(&mut data[2], (t * 64 + k) * 32 + n, r::f(0x1000));
            }
        }
    }
    (job, data)
}

pub(super) const BOUNDARIES: [([usize; 2], u16); 7] = [
    ([7, 8], 0x3c01),
    ([15, 16], 0x3c00),
    ([23, 24], 0x3c01),
    ([31, 32], 0x3c00),
    ([47, 48], 0x3c00),
    ([55, 56], 0x3c01),
    ([0, 63], 0x3c00),
];

#[test]
fn up64_rounds_at_each_sixteen_boundary_and_carries_skip_as_c() {
    for (pair, want) in BOUNDARIES {
        let (job, data) = boundary_witness(pair);
        assert_eq!(job.args()[6], 16);
        let out = r::run(job, data.each_ref().map(|v| v.as_slice()));
        for y in 0..2 {
            for x in 0..2 {
                for n in 0..32 {
                    let got = r::h(r::get(&out, r::addr(x, y, 2, 32, n)));
                    assert_eq!(got, want, "{pair:?}");
                }
            }
        }
    }
}

#[test]
fn down16_tap_boundary_is_a_helper_boundary() {
    let job = Job::new(Kernel::Down16, 1, 1);
    let mut data = job.extents().unwrap().map(|n| vec![0; (n / 4) as usize]);
    for y in 0..2 {
        for x in 0..2 {
            for k in 0..16 {
                r::put(&mut data[0], r::addr(x, y, 2, 16, k), 1.0);
            }
        }
    }
    for (terms, want) in [([(0, 15), (1, 0)], 0x3c00), ([(1, 7), (1, 8)], 0x3c01)] {
        data[2].fill(0);
        for n in 0..32 {
            r::put(&mut data[2], n, 1.0);
            for (t, k) in terms {
                r::put(&mut data[2], (t * 16 + k) * 32 + n, r::f(0x1000));
            }
        }
        let out = r::run(job, data.each_ref().map(|v| v.as_slice()));
        for n in 0..32 {
            assert_eq!(r::h(r::get(&out, r::addr(0, 0, 1, 32, n))), want);
        }
    }
}

#[test]
fn every_native_matrix_job_selects_the_translated_contract() {
    for kernel in [
        Kernel::Conv16,
        Kernel::Conv32,
        Kernel::Conv64,
        Kernel::Down16,
        Kernel::Down32,
        Kernel::Up64,
        Kernel::Up32,
        Kernel::Out,
        Kernel::Resample,
    ] {
        let job = Job::new(kernel, 6, 6);
        assert_eq!(job.args()[6], 16, "{kernel:?}");
    }
}
