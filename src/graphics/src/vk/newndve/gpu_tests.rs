use super::{
    kernels::{Job, Kernels},
    reference as r,
    tests::{fixture, rounding_witness},
    Kernel,
};
use crate::vk::{queue, Gpu};
use ash::vk;
use gpu_allocator::MemoryLocation;

fn execute(g: &Gpu, kernels: &Kernels<'_>, job: Job, data: &[Vec<u32>; 5]) -> Vec<u32> {
    let buffers: Vec<_> = data
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let b = g
                .buffer(
                    (v.len() * 4 + 64) as u64,
                    vk::BufferUsageFlags::STORAGE_BUFFER,
                    MemoryLocation::CpuToGpu,
                    "NDVE synthetic guarded buffer",
                )
                .unwrap();
            assert!(!b.ptr.is_null());
            unsafe {
                std::ptr::copy_nonoverlapping(v.as_ptr().cast::<u8>(), b.ptr, v.len() * 4);
                std::ptr::write_bytes(b.ptr.add(v.len() * 4), 0xa5, 64);
            }
            if i == 1 && job.kernel == Kernel::Compare {
                unsafe {
                    *b.ptr.cast::<u32>().add(1) = u32::MAX;
                }
            }
            b
        })
        .collect();
    let descriptors = std::array::from_fn(|i| {
        vk::DescriptorBufferInfo::default()
            .buffer(buffers[i].buf)
            .offset(0)
            .range(buffers[i].size)
    });
    let ticket = queue::oneoff(g, |g, sub, _| {
        let mut short = descriptors;
        short[0].range = 0;
        assert!(kernels.record(sub, job, short).is_err());
        let mut alias = descriptors;
        alias[0] = alias[1];
        assert!(kernels.record(sub, job, alias).is_err());
        kernels.record(sub, job, descriptors).unwrap();
        unsafe {
            g.dev().cmd_pipeline_barrier(
                sub.main,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::PipelineStageFlags::HOST,
                vk::DependencyFlags::empty(),
                &[vk::MemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::SHADER_WRITE)
                    .dst_access_mask(vk::AccessFlags::HOST_READ)],
                &[],
                &[],
            );
        }
    });
    // Never release in-flight resources after a timed-out wait. The test is run as
    // an isolated process under the outer watchdog as well as this finite fence.
    if !queue::wait_fence(ticket, 5_000_000_000) {
        eprintln!("NDVE synthetic GPU fence exceeded 5 seconds");
        std::process::exit(70);
    }
    for (i, b) in buffers.iter().enumerate() {
        let tail = unsafe { std::slice::from_raw_parts(b.ptr.add(data[i].len() * 4), 64) };
        assert!(
            tail.iter().all(|&v| v == 0xa5),
            "{:?} buffer {i} guard overwritten",
            job.kernel
        );
        if i != 1 {
            let got = unsafe { std::slice::from_raw_parts(b.ptr.cast::<u32>(), data[i].len()) };
            assert_eq!(got, data[i], "input modified");
        }
    }
    let got =
        unsafe { std::slice::from_raw_parts(buffers[1].ptr.cast::<u32>(), data[1].len()) }.to_vec();
    for b in buffers {
        g.free_buffer(b);
    }
    got
}

#[test]
#[ignore = "bounded synthetic Vulkan NDVE certification; run in an isolated process"]
fn synthetic_gpu_parity_and_translated_rounding_discriminator() {
    std::env::set_var("VK_LOADER_LAYERS_DISABLE", "~implicit~");
    std::env::set_var("NXGFX_VALIDATION", "1");
    let g = Gpu::get().unwrap();
    let before = crate::vk::validation_counts();
    let start = std::time::Instant::now();
    let kernels = Kernels::new(g).unwrap();
    eprintln!("NDVE native pipelines built in {:?}", start.elapsed());
    let mut words = 0;
    for kernel in [
        Kernel::Pack,
        Kernel::Conv16,
        Kernel::Conv32,
        Kernel::Conv64,
        Kernel::Down16,
        Kernel::Down32,
        Kernel::Up64,
        Kernel::Up32,
        Kernel::Out,
        Kernel::Luma,
        Kernel::Resample,
        Kernel::Compare,
    ] {
        let mut job = if kernel == Kernel::Resample {
            Job::new(kernel, 24, 12)
        } else {
            Job::new(kernel, 4, 3)
        };
        job.dither = [8, 8];
        let mut data = fixture(job);
        if kernel == Kernel::Compare {
            data[4] = data[0].clone();
            data[4][7] ^= 1;
        }
        let want = r::run(job, data.each_ref().map(|v| v.as_slice()));
        let got = execute(g, &kernels, job, &data);
        let bad = got.iter().zip(&want).filter(|(a, b)| a != b).count();
        let first = got.iter().zip(&want).position(|(a, b)| a != b);
        eprintln!(
            "NDVE {kernel:?}: {} packed words, {bad} differences, first {first:?}",
            got.len()
        );
        if let Some(i) = first {
            eprintln!("at {i}: GPU={:08x}, CPU={:08x}", got[i], want[i]);
        }
        assert_eq!(bad, 0, "{kernel:?}");
        words += got.len();
        if kernel == Kernel::Resample {
            job.flip = true;
            let want = r::run(job, data.each_ref().map(|v| v.as_slice()));
            assert_eq!(
                execute(g, &kernels, job, &data),
                want,
                "resample vertical flip"
            );
            words += want.len();
        }
    }
    // Exact compatibility witness; retain the old eight-product result separately.
    let (job, data) = rounding_witness();
    let native = execute(g, &kernels, job, &data);
    for y in 0..2 {
        for x in 0..2 {
            for n in 0..32 {
                assert_eq!(r::h(r::get(&native, r::addr(x, y, 2, 32, n))), 0x3c01);
            }
        }
    }
    let eight = r::run_eight_product_comparison(job, data.each_ref().map(|v| v.as_slice()));
    for y in 0..2 {
        for x in 0..2 {
            for n in 0..32 {
                assert_eq!(r::h(r::get(&eight, r::addr(x, y, 2, 32, n))), 0x3c00);
            }
        }
    }
    for (pair, want) in super::contract_tests::BOUNDARIES {
        let (job, data) = super::contract_tests::boundary_witness(pair);
        let got = execute(g, &kernels, job, &data);
        for y in 0..2 {
            for x in 0..2 {
                for n in 0..32 {
                    assert_eq!(
                        r::h(r::get(&got, r::addr(x, y, 2, 32, n))),
                        want,
                        "boundary {pair:?}"
                    );
                }
            }
        }
    }
    // Require the CPU target's subnormal preservation through multiply, half
    // conversion, ReLU and storage. UnpackHalf2x16 permits device-dependent FTZ;
    // a failure is an uncertified FP contract, not permission to weaken parity.
    let mut tiny = data.clone();
    tiny[2].fill(0);
    tiny[4].fill(0);
    for t in 0..4 {
        for n in 0..32 {
            r::put(&mut tiny[2], t * 64 * 32 + n, r::f(1));
        }
    }
    let got = execute(g, &kernels, job, &tiny);
    for y in 0..2 {
        for x in 0..2 {
            for n in 0..32 {
                assert_eq!(
                    r::h(r::get(&got, r::addr(x, y, 2, 32, n))),
                    1,
                    "fp16 subnormal flushed"
                );
            }
        }
    }
    eprintln!("NDVE fp16 subnormal witness: 128/128 outputs preserved 0x0001");
    // Independent lane/fragment shader implements the translator helper, rather
    // than changing a rounding knob on the kernel being tested.
    const ORACLE: &[u8] = include_bytes!(concat!(
        env!("OUT_DIR"),
        "/test_spv/ndve_hmma_translated_comp.spv"
    ));
    let oracle = Kernels::from_sources(g, [ORACLE; 4]).unwrap();
    let mut matrix = [
        vec![0; 128],
        vec![0; 64],
        vec![0; 64],
        vec![0; 4],
        vec![0; 128],
    ];
    for i in 0..256 {
        r::put(&mut matrix[0], i, 1.0);
    }
    for i in 0..128 {
        r::put(&mut matrix[4], i, 1.0);
    }
    for k in [0, 8] {
        for n in 0..8 {
            r::put(&mut matrix[2], k * 8 + n, 2f32.powi(-11));
        }
    }
    let translated = execute(g, &oracle, Job::new(Kernel::Compare, 16, 8), &matrix);
    for i in 0..128 {
        let at = r::addr((i / 32) % 2, (i / 32) / 2, 2, 32, i % 32);
        assert_eq!(
            r::h(r::get(&translated, i)),
            0x3c01,
            "translated fragment {i}"
        );
        assert_eq!(
            r::get(&native, at),
            r::get(&translated, i),
            "native/fragment {i}"
        );
    }
    eprintln!("NDVE retained-helper witness: 128/128 native/translated halves=3c01; eight-product comparison=3c00 (1 ULP); guest replacement remains off");
    words += graph_parity(g, &kernels);
    assert_eq!(
        crate::vk::validation_counts(),
        before,
        "Vulkan validation diagnostics"
    );
    eprintln!("NDVE synthetic: {words} native packed words matched CPU; guards intact; zero new Vulkan validation errors/warnings; {:?}",start.elapsed());
}

/// The complete model in one GPU submission: verifies its skip edges, reused
/// tensor lifetimes and compute barriers as well as the individual arithmetic.
fn graph_parity(g: &Gpu, kernels: &Kernels<'_>) -> usize {
    use super::LAYERS;
    let shapes = [
        [24, 12, 16],
        [24, 12, 16],
        [24, 12, 16],
        [12, 6, 32],
        [12, 6, 32],
        [12, 6, 32],
        [6, 3, 64],
        [6, 3, 64],
    ];
    let mut cpu: Vec<Vec<u32>> = shapes
        .iter()
        .map(|&[w, h, c]| vec![0; ((w + 2) * (h + 2 + 16) * c / 2) as usize])
        .collect();
    let mut pic = vec![0; 48 * 24];
    for (i, v) in pic.iter_mut().enumerate() {
        *v = (i as u32).wrapping_mul(0x719b12d3) ^ 0x6a358fc1;
    }
    let alloc = |v: &[u32]| {
        let b = g
            .buffer(
                (v.len() * 4 + 64) as u64,
                vk::BufferUsageFlags::STORAGE_BUFFER,
                MemoryLocation::CpuToGpu,
                "NDVE synthetic graph",
            )
            .unwrap();
        unsafe {
            std::ptr::copy_nonoverlapping(v.as_ptr().cast::<u8>(), b.ptr, v.len() * 4);
            std::ptr::write_bytes(b.ptr.add(v.len() * 4), 0xa5, 64);
        }
        b
    };
    let tensors: Vec<_> = cpu.iter().map(|v| alloc(v)).collect();
    let picture = alloc(&pic);
    let output = alloc(&vec![0; pic.len()]);
    let dummy = alloc(&[0; 4]);
    let mut jobs = Vec::new();
    let mut parameters = Vec::new();
    let mut final_cpu = Vec::new();
    for layer in LAYERS {
        let shape = shapes[if layer.input == 8 { 0 } else { layer.input }];
        let down = matches!(layer.kernel, Kernel::Down16 | Kernel::Down32);
        let job = Job::new(
            layer.kernel,
            if down { shape[0] / 2 } else { shape[0] },
            if down { shape[1] / 2 } else { shape[1] },
        );
        let generated = fixture(job);
        parameters.push((alloc(&generated[2]), alloc(&generated[3])));
        let input = if layer.input == 8 {
            &pic
        } else {
            &cpu[layer.input]
        };
        let skip = if let Some(s) = layer.skip {
            &cpu[s]
        } else if layer.kernel == Kernel::Out {
            &pic
        } else {
            &generated[4]
        };
        let result = r::run(job, [input, &[], &generated[2], &generated[3], skip]);
        if layer.output == 8 {
            final_cpu = result;
        } else {
            cpu[layer.output] = result;
        }
        jobs.push(job);
    }
    let descriptor = |b: &crate::vk::Buf| {
        vk::DescriptorBufferInfo::default()
            .buffer(b.buf)
            .range(b.size)
    };
    let ticket = queue::oneoff(g, |g, sub, _| {
        for (i, layer) in LAYERS.iter().enumerate() {
            let input = if layer.input == 8 {
                &picture
            } else {
                &tensors[layer.input]
            };
            let out = if layer.output == 8 {
                &output
            } else {
                &tensors[layer.output]
            };
            let skip = if let Some(s) = layer.skip {
                &tensors[s]
            } else if layer.kernel == Kernel::Out {
                &picture
            } else {
                &dummy
            };
            kernels
                .record(
                    sub,
                    jobs[i],
                    [
                        descriptor(input),
                        descriptor(out),
                        descriptor(&parameters[i].0),
                        descriptor(&parameters[i].1),
                        descriptor(skip),
                    ],
                )
                .unwrap();
        }
        unsafe {
            g.dev().cmd_pipeline_barrier(
                sub.main,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::PipelineStageFlags::HOST,
                vk::DependencyFlags::empty(),
                &[vk::MemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::SHADER_WRITE)
                    .dst_access_mask(vk::AccessFlags::HOST_READ)],
                &[],
                &[],
            );
        }
    });
    if !queue::wait_fence(ticket, 5_000_000_000) {
        std::process::exit(70);
    }
    let mut words = 0;
    for (i, (b, want)) in tensors
        .iter()
        .zip(&cpu)
        .chain(std::iter::once((&output, &final_cpu)))
        .enumerate()
    {
        let got = unsafe { std::slice::from_raw_parts(b.ptr.cast::<u32>(), want.len()) };
        let bad = got.iter().zip(want).filter(|(a, b)| a != b).count();
        assert_eq!(bad, 0, "graph tensor {i}");
        let tail = unsafe { std::slice::from_raw_parts(b.ptr.add(want.len() * 4), 64) };
        assert!(tail.iter().all(|&v| v == 0xa5), "graph tensor {i} guard");
        words += want.len();
    }
    for b in tensors {
        g.free_buffer(b);
    }
    for (w, b) in parameters {
        g.free_buffer(w);
        g.free_buffer(b);
    }
    for b in [picture, output, dummy] {
        g.free_buffer(b);
    }
    eprintln!("NDVE complete 13-dispatch synthetic graph: {words} packed words match, including all 8 final tensors and RGB10A2 output");
    words
}
