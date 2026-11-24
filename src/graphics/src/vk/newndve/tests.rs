use super::{kernels::Job, reference as r, *};

pub(super) fn fixture(job: Job) -> [Vec<u32>; 5] {
    let mut buffers = job.extents().unwrap().map(|n| vec![0; (n / 4) as usize]);
    let mut seed = 0x19770527u32;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    let [t, k, n] = job.kernel.shape();
    let mut fill = |v: &mut [u32], scale: f32| {
        for i in 0..v.len() * 2 {
            r::put(
                v,
                i,
                ((next() % 20001) as i32 - 10000) as f32 / 10000.0 * scale,
            );
        }
    };
    for i in [0, 4] {
        fill(&mut buffers[i], 1.0);
    }
    fill(
        &mut buffers[2],
        if t == 0 { 0.35 } else { 3.0 / (t * k) as f32 },
    );
    fill(&mut buffers[3], 0.05);
    let down = matches!(job.kernel, Kernel::Down16 | Kernel::Down32);
    let up = matches!(job.kernel, Kernel::Up64 | Kernel::Up32);
    let w = job.width as usize;
    let h = job.height as usize;
    // The border is zero even for random inputs. Catch accidentally wrapped reads.
    for (idx, tw, th, c) in [
        (
            0,
            if down { 2 * w } else { w },
            if down { 2 * h } else { h },
            k as usize,
        ),
        (4, 2 * w, 2 * h, if up { n as usize } else { 0 }),
    ] {
        if c == 0 {
            continue;
        }
        for y in 0..th + 2 {
            for x in 0..tw + 2 {
                if x == 0 || x == tw + 1 || y == 0 || y == th + 1 {
                    for ch in 0..c {
                        r::put(&mut buffers[idx], (y * (tw + 2) + x) * c + ch, 0.0);
                    }
                }
            }
        }
    }
    match job.kernel {
        Kernel::Pack | Kernel::Luma => {
            for v in &mut buffers[0] {
                *v = next();
            }
        }
        Kernel::Out => {
            for v in &mut buffers[4] {
                *v = next();
            }
        }
        Kernel::Resample => {
            for v in &mut buffers[3] {
                *v = next() | 0xc0000000;
            }
            for v in &mut buffers[4] {
                *v = next() | 0xc0000000;
            }
            // The resampler's luma is from the actual picture, rather than unrelated data.
            let lj = Job::new(Kernel::Luma, job.width, job.height);
            buffers[0] = r::run(lj, [&buffers[4], &[], &[], &[], &[]]);
        }
        _ => {}
    }
    buffers
}

#[test]
fn half_rounding_ties_subnormals_overflow_and_all_finite_encodings() {
    for h in 0..=u16::MAX {
        if h & 0x7c00 != 0x7c00 {
            assert_eq!(r::h(r::f(h)), h);
        }
    }
    for (f, h) in [
        (1.0 + 2f32.powi(-11), 0x3c00),
        (1.0 + 3.0 * 2f32.powi(-11), 0x3c02),
        (2f32.powi(-25), 0),
        (3.0 * 2f32.powi(-25), 2),
        (65520.0, 0x7c00),
        (-0.0, 0x8000),
    ] {
        assert_eq!(r::h(f), h, "{f}");
    }
}

#[test]
fn model_offsets_and_end_are_exact() {
    assert_eq!(
        offsets().map(|o| o.0),
        [
            0, 0x1300, 0x2600, 0x3700, 0x8000, 0xc100, 0x1e200, 0x30300, 0x34400, 0x38d00, 0x39e00,
            0x3b100
        ]
    );
    assert_eq!(
        offsets().map(|o| o.1),
        [
            0x1200, 0x2500, 0x3600, 0x7f00, 0xc000, 0x1e100, 0x30200, 0x34300, 0x38c00, 0x39d00,
            0x3b000, 0x3c300
        ]
    );
    assert_eq!(offsets()[11].1 + 32, WEIGHT_BYTES);
}

#[test]
fn every_prefix_and_deviation_is_rejected_without_consumption() {
    let table = Span {
        address: 0x10000,
        size: WEIGHT_BYTES,
    };
    let good: Vec<_> = offsets()
        .into_iter()
        .enumerate()
        .map(|(i, (w, b))| {
            let [t, k, n] = LAYERS[i + 1].kernel.shape();
            Dispatch {
                key: KEYS[i + 1],
                weights: Span {
                    address: table.address + w,
                    size: u64::from(t * k * n * 2),
                },
                bias: Span {
                    address: table.address + b,
                    size: u64::from(n * 2),
                },
                groups: [1, 1, 1],
                indirect: false,
            }
        })
        .collect();
    assert!(validate_sequence(&good, table).is_ok());
    for len in 0..12 {
        assert!(validate_sequence(&good[..len], table).is_err());
    }
    for i in 0..12 {
        for mutation in 0..6 {
            let mut ds = good.clone();
            match mutation {
                0 => ds[i].key ^= 1,
                1 => ds[i].weights.size -= 1,
                2 => ds[i].bias.size -= 1,
                3 => ds[i].bias.address += 256,
                4 => ds[i].indirect = true,
                _ => ds[i].groups[1] = 0,
            }
            assert!(
                validate_sequence(&ds, table).is_err(),
                "layer {i}, mutation {mutation}"
            );
        }
    }
    assert!(!Span {
        address: u64::MAX - 4,
        size: 8
    }
    .contains(u64::MAX - 4, 8));
    assert!(validate_sequence(
        &good,
        Span {
            address: table.address,
            size: WEIGHT_BYTES - 1
        }
    )
    .is_err());
}

#[test]
fn owner_hold_refuses_all_live_modes_without_consuming_guest_dispatches() {
    assert_eq!(Mode::parse(None), Ok(Mode::Off));
    assert_eq!(Mode::parse(Some("off")), Ok(Mode::Off));
    assert_eq!(Mode::parse(Some("0")), Ok(Mode::Off));
    assert!(Mode::parse(Some("check")).is_err());
    assert!(Mode::parse(Some("on")).is_err());
    assert!(Mode::parse(Some("1")).is_err());
    assert!(Mode::parse(Some("ON")).is_err());
    assert!(Mode::parse(Some("")).is_err());
}

#[test]
fn frame_contract_rejects_distinct_images_aliasing_guest_storage_and_unknown_generation() {
    use ash::vk;
    let image = |id, width, height, format: vk::Format| ImageBinding {
        id, serial: id as u64, generation: id as u64, image: id as u64,
        width, height, format: format.as_raw(), sampled_view: id as u64 + 100,
        storage_view: id as u64 + 200,
        usage: (vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::STORAGE
            | vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::TRANSFER_DST).as_raw(),
        guest: Span { address: id as u64 * 0x1000000, size: 0xf00000 },
    };
    let mut frame = FrameResources {
        picture: image(1, 1920, 1080, vk::Format::R8G8B8A8_UNORM),
        network_output: image(2, 1920, 1080, vk::Format::A2B10G10R10_UNORM_PACK32),
        dither: image(3, 512, 512, vk::Format::A2B10G10R10_UNORM_PACK32),
        final_output: image(4, 2560, 1440, vk::Format::R8G8B8A8_SRGB),
        resample_weights: Span { address: 0x10000, size: 0x3000 }, constants: [0; 96],
    };
    for (word, value) in [(2, 1919i32), (3, 1079), (6, 2559), (7, 1439), (20, 1)] {
        frame.constants[word * 4..word * 4 + 4].copy_from_slice(&value.to_le_bytes());
    }
    assert!(frame.validate(1).is_ok());
    let original = frame;
    frame.final_output.guest = frame.picture.guest;
    assert!(frame.validate(1).is_err());
    frame = original;
    frame.network_output.generation = 0;
    assert!(frame.validate(1).is_err());
    frame = original;
    frame.dither.guest = Span { address: u64::MAX - 4, size: 8 };
    assert!(frame.validate(1).is_err());
}

pub(super) fn rounding_witness() -> (Job, [Vec<u32>; 5]) {
    let job = Job::new(Kernel::Up64, 1, 1);
    let mut bs = job.extents().unwrap().map(|n| vec![0; (n / 4) as usize]);
    for k in 0..64 {
        r::put(&mut bs[0], r::addr(0, 0, 1, 64, k), 1.0);
    }
    for y in 0..2 {
        for x in 0..2 {
            for n in 0..32 {
                r::put(&mut bs[4], r::addr(x, y, 2, 32, n), 1.0);
            }
        }
    }
    for t in 0..4 {
        for n in 0..32 {
            for k in [0, 8] {
                r::put(&mut bs[2], (t * 64 + k) * 32 + n, 2f32.powi(-11));
            }
        }
    }
    (job, bs)
}

#[test]
fn hmma_eight_product_rounding_differs_from_current_translated_sixteen() {
    let (job, bs) = rounding_witness();
    // The pre-DS-01 native job supplied 8 here: that wiring must fail this test.
    assert_eq!(job.args()[6], 16);
    let eight = r::run_eight_product_comparison(job, bs.each_ref().map(|v| v.as_slice()));
    let translated = r::run(job, bs.each_ref().map(|v| v.as_slice()));
    let mut differences = 0;
    for y in 0..2 {
        for x in 0..2 {
            for n in 0..32 {
                let at = r::addr(x, y, 2, 32, n);
                assert_eq!(r::h(r::get(&eight, at)), 0x3c00);
                assert_eq!(r::h(r::get(&translated, at)), 0x3c01);
                differences += usize::from(r::get(&eight, at) != r::get(&translated, at));
            }
        }
    }
    assert_eq!(
        differences, 128,
        "preserve the exact historical one-ULP witness"
    );
}

#[test]
fn pack_and_out_have_distinct_channel_orders() {
    let pack = Job::new(Kernel::Pack, 1, 1);
    let mut bs = pack.extents().unwrap().map(|n| vec![0; (n / 4) as usize]);
    bs[0] = vec![0x03020100, 0x07060504, 0x0b0a0908, 0x0f0e0d0c];
    let out = r::run(pack, bs.each_ref().map(|v| v.as_slice()));
    for n in 0..16 {
        assert_eq!(
            r::get(&out, r::addr(0, 0, 1, 16, n)),
            r::round(n as f32 / 255.0)
        );
    }
    let job = Job::new(Kernel::Out, 1, 1);
    let mut bs = job.extents().unwrap().map(|n| vec![0; (n / 4) as usize]);
    for n in 0..16 {
        r::put(&mut bs[3], n, n as f32 / 32.0);
    }
    let out = r::run(job, bs.each_ref().map(|v| v.as_slice()));
    for sub in 0..4 {
        let pix = r::rgb10(out[sub]);
        for c in 0..3 {
            let n = [2 * sub, 2 * sub + 1, 8 + 2 * sub][c];
            assert!((pix[c] - n as f32 / 32.0).abs() <= 0.5 / 1023.0);
        }
    }
}

#[test]
fn kernel_jobs_reject_unsafe_dimensions_before_allocation() {
    assert!(Job::new(Kernel::Conv64, u32::MAX, 1).extents().is_err());
    assert!(Job::new(Kernel::Conv16, 0, 1).extents().is_err());
    assert!(Job::new(Kernel::Luma, 3, 3).extents().is_err());
    assert!(Job::new(Kernel::Resample, 7, 6).extents().is_err());
    assert!(Job::new(Kernel::Resample, 1920, 1080).extents().is_ok());
}

#[test]
fn production_tensor_and_image_extents_match_the_guest_contract() {
    let tensor = |w: u64, h: u64, c: u64| (w + 2) * (h + 2 + 16) * c * 2;
    assert_eq!(
        Job::new(Kernel::Pack, 960, 540).extents().unwrap()[1],
        tensor(960, 540, 16)
    );
    assert_eq!(
        Job::new(Kernel::Down16, 480, 270).extents().unwrap()[0],
        tensor(960, 540, 16)
    );
    assert_eq!(
        Job::new(Kernel::Up64, 240, 135).extents().unwrap()[1],
        tensor(480, 270, 32)
    );
    assert_eq!(
        Job::new(Kernel::Out, 960, 540).extents().unwrap()[1],
        1920 * 1080 * 4
    );
    assert_eq!(
        Job::new(Kernel::Resample, 1920, 1080).extents().unwrap()[1],
        2560 * 1440 * 4
    );
    assert_eq!(r::addr(0, 0, 960, 16, 0), (960 + 2) * 16 + 16);
    assert_eq!(
        r::addr(959, 539, 960, 16, 15),
        ((540 * 962 + 960) * 16) + 15
    );
}
