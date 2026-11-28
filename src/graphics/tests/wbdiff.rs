//! The write-back compare kernel (`vk::wbdiff`, OOT-FPS) on the GPU: a range starting mid-page over six pages, the GPU
//! changing one word in two of its whole pages. Only those pages and the partial first and last ones are copied back,
//! with the right bytes; the flags say which. `cargo test --release -p graphics --features gpu-tests --test wbdiff`.
#![cfg(feature = "gpu-tests")]

mod common;

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
fn only_changed_and_partial_pages_come_back() {
    let _l = common::setup();
    let e0 = common::validation_errors();
    let g = gpu();
    const PAGE: usize = 4096;
    let a: u64 = 0x1_0000_0000 + 100;
    let n = 6 * PAGE - 200;
    let before: Vec<u8> = (0..n).map(|i| (i * 7 % 251) as u8).collect();
    let mut after = before.clone();
    // pages 2 and 4 of the range's six (offsets from `a`: page p starts at p * PAGE - 100)
    for p in [2usize, 4] {
        after[p * PAGE - 100 + 40] ^= 0x81;
    }
    let (flags, rb, bf) = nxgfx::vk::wbdiff::compare_once(g, a, &after, &before).expect("kernel");
    assert_eq!(flags, vec![2, 1, 2, 1, 2, 2], "partial first and last, changed 2 and 4");
    for p in 0..6usize {
        let lo = (p * PAGE).saturating_sub(100);
        let hi = ((p + 1) * PAGE - 100).min(n);
        if flags[p] == 2 {
            assert_eq!(rb[lo..hi], after[lo..hi], "page {p}: the GPU's bytes");
            assert_eq!(bf[lo..hi], before[lo..hi], "page {p}: the baseline");
        } else {
            assert!(rb[lo..hi].iter().all(|&b| b == 0xee), "page {p}: not copied");
            assert!(bf[lo..hi].iter().all(|&b| b == 0x11), "page {p}: not copied");
        }
    }
    assert_eq!(common::validation_errors(), e0, "validation errors");
}
