//! ASTC on the GPU. Desktop GPUs do not sample ASTC: the blocks are decoded by a compute shader into RGBA8 texels
//! (`shaders/astc_decode.comp`, the same bytes as astcenc's 8-bit LDR decode), and textures of 256x256 and up are
//! re-encoded to BC7 (`shaders/bc7_encode.comp`, modes 1, 3, 6 and 7): 1 byte per texel instead of 4. Encoded BC7
//! levels go into the disk cache (`astc_cache`) and later uploads of the same ASTC bytes take them from there.

use crate::vk::queue::Sub;
use crate::vk::Gpu;
use ash::vk;
use gpu_allocator::MemoryLocation;

static DECODE_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/spv/astc_decode_comp.spv"));
static BC7_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/spv/bc7_encode_comp.spv"));
static BC7_M6_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/spv/bc7_encode_comp_m6.spv"));
/// The full BC7 encoder's pipeline (0 until made): its pipeline takes seconds to make the first time, so it is made on a
/// thread of its own (through the pipeline cache, which keeps it for later runs) while the mode-6 build encodes.
static BC7_FULL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

// The unquantization tables the decoder reads (binding 2): 12 + 16 row offsets in bytes, then the rows.
static WUNQ: [u8; 142] = [
    0, 64, 0, 32, 64, 0, 21, 43, 64, 0, 16, 32, 48, 64, 0, 64, 12, 52, 25, 39, 0, 9, 18, 27, 37,
    46, 55, 64, 0, 64, 7, 57, 14, 50, 21, 43, 28, 36, 0, 64, 17, 47, 5, 59, 23, 41, 11, 53, 28, 36,
    0, 4, 8, 12, 17, 21, 25, 29, 35, 39, 43, 47, 52, 56, 60, 64, 0, 64, 16, 48, 3, 61, 19, 45, 6,
    58, 23, 41, 9, 55, 26, 38, 13, 51, 29, 35, 0, 64, 8, 56, 16, 48, 24, 40, 2, 62, 11, 53, 19, 45,
    27, 37, 5, 59, 13, 51, 22, 42, 30, 34, 0, 2, 4, 6, 8, 10, 12, 14, 16, 18, 20, 22, 24, 26, 28,
    30, 34, 36, 38, 40, 42, 44, 46, 48, 50, 52, 54, 56, 58, 60, 62, 64,
];
static WUNQ_OFF: [u16; 12] = [0, 2, 5, 9, 14, 20, 28, 38, 50, 66, 86, 110];
static CUNQ_OFF: [u16; 16] = [
    0, 6, 14, 24, 36, 52, 72, 96, 128, 168, 216, 280, 360, 456, 584, 744,
];

/// Colour unquantization rows (QUANT_6 .. QUANT_192), the ASTC specification's values in integer-sequence order.
static CUNQ: [u8; 936] = [
    0, 255, 51, 204, 102, 153, 0, 36, 73, 109, 146, 182, 219, 255, 0, 255, 28, 227, 56, 199, 84,
    171, 113, 142, 0, 255, 69, 186, 23, 232, 92, 163, 46, 209, 116, 139, 0, 17, 34, 51, 68, 85,
    102, 119, 136, 153, 170, 187, 204, 221, 238, 255, 0, 255, 67, 188, 13, 242, 80, 175, 27, 228,
    94, 161, 40, 215, 107, 148, 54, 201, 121, 134, 0, 255, 33, 222, 66, 189, 99, 156, 11, 244, 44,
    211, 77, 178, 110, 145, 22, 233, 55, 200, 88, 167, 121, 134, 0, 8, 16, 24, 33, 41, 49, 57, 66,
    74, 82, 90, 99, 107, 115, 123, 132, 140, 148, 156, 165, 173, 181, 189, 198, 206, 214, 222, 231,
    239, 247, 255, 0, 255, 32, 223, 65, 190, 97, 158, 6, 249, 39, 216, 71, 184, 104, 151, 13, 242,
    45, 210, 78, 177, 110, 145, 19, 236, 52, 203, 84, 171, 117, 138, 26, 229, 58, 197, 91, 164,
    123, 132, 0, 255, 16, 239, 32, 223, 48, 207, 65, 190, 81, 174, 97, 158, 113, 142, 5, 250, 21,
    234, 38, 217, 54, 201, 70, 185, 86, 169, 103, 152, 119, 136, 11, 244, 27, 228, 43, 212, 59,
    196, 76, 179, 92, 163, 108, 147, 124, 131, 0, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 44, 48, 52,
    56, 60, 65, 69, 73, 77, 81, 85, 89, 93, 97, 101, 105, 109, 113, 117, 121, 125, 130, 134, 138,
    142, 146, 150, 154, 158, 162, 166, 170, 174, 178, 182, 186, 190, 195, 199, 203, 207, 211, 215,
    219, 223, 227, 231, 235, 239, 243, 247, 251, 255, 0, 255, 16, 239, 32, 223, 48, 207, 64, 191,
    80, 175, 96, 159, 112, 143, 3, 252, 19, 236, 35, 220, 51, 204, 67, 188, 83, 172, 100, 155, 116,
    139, 6, 249, 22, 233, 38, 217, 54, 201, 71, 184, 87, 168, 103, 152, 119, 136, 9, 246, 25, 230,
    42, 213, 58, 197, 74, 181, 90, 165, 106, 149, 122, 133, 13, 242, 29, 226, 45, 210, 61, 194, 77,
    178, 93, 162, 109, 146, 125, 130, 0, 255, 8, 247, 16, 239, 24, 231, 32, 223, 40, 215, 48, 207,
    56, 199, 64, 191, 72, 183, 80, 175, 88, 167, 96, 159, 104, 151, 112, 143, 120, 135, 2, 253, 10,
    245, 18, 237, 26, 229, 35, 220, 43, 212, 51, 204, 59, 196, 67, 188, 75, 180, 83, 172, 91, 164,
    99, 156, 107, 148, 115, 140, 123, 132, 5, 250, 13, 242, 21, 234, 29, 226, 37, 218, 45, 210, 53,
    202, 61, 194, 70, 185, 78, 177, 86, 169, 94, 161, 102, 153, 110, 145, 118, 137, 126, 129, 0, 2,
    4, 6, 8, 10, 12, 14, 16, 18, 20, 22, 24, 26, 28, 30, 32, 34, 36, 38, 40, 42, 44, 46, 48, 50,
    52, 54, 56, 58, 60, 62, 64, 66, 68, 70, 72, 74, 76, 78, 80, 82, 84, 86, 88, 90, 92, 94, 96, 98,
    100, 102, 104, 106, 108, 110, 112, 114, 116, 118, 120, 122, 124, 126, 129, 131, 133, 135, 137,
    139, 141, 143, 145, 147, 149, 151, 153, 155, 157, 159, 161, 163, 165, 167, 169, 171, 173, 175,
    177, 179, 181, 183, 185, 187, 189, 191, 193, 195, 197, 199, 201, 203, 205, 207, 209, 211, 213,
    215, 217, 219, 221, 223, 225, 227, 229, 231, 233, 235, 237, 239, 241, 243, 245, 247, 249, 251,
    253, 255, 0, 255, 8, 247, 16, 239, 24, 231, 32, 223, 40, 215, 48, 207, 56, 199, 64, 191, 72,
    183, 80, 175, 88, 167, 96, 159, 104, 151, 112, 143, 120, 135, 1, 254, 9, 246, 17, 238, 25, 230,
    33, 222, 41, 214, 49, 206, 57, 198, 65, 190, 73, 182, 81, 174, 89, 166, 97, 158, 105, 150, 113,
    142, 121, 134, 3, 252, 11, 244, 19, 236, 27, 228, 35, 220, 43, 212, 51, 204, 59, 196, 67, 188,
    75, 180, 83, 172, 91, 164, 99, 156, 107, 148, 115, 140, 123, 132, 4, 251, 12, 243, 20, 235, 28,
    227, 36, 219, 44, 211, 52, 203, 60, 195, 68, 187, 76, 179, 84, 171, 92, 163, 100, 155, 108,
    147, 116, 139, 124, 131, 6, 249, 14, 241, 22, 233, 30, 225, 38, 217, 46, 209, 54, 201, 62, 193,
    70, 185, 78, 177, 86, 169, 94, 161, 102, 153, 110, 145, 118, 137, 126, 129, 0, 255, 4, 251, 8,
    247, 12, 243, 16, 239, 20, 235, 24, 231, 28, 227, 32, 223, 36, 219, 40, 215, 44, 211, 48, 207,
    52, 203, 56, 199, 60, 195, 64, 191, 68, 187, 72, 183, 76, 179, 80, 175, 84, 171, 88, 167, 92,
    163, 96, 159, 100, 155, 104, 151, 108, 147, 112, 143, 116, 139, 120, 135, 124, 131, 1, 254, 5,
    250, 9, 246, 13, 242, 17, 238, 21, 234, 25, 230, 29, 226, 33, 222, 37, 218, 41, 214, 45, 210,
    49, 206, 53, 202, 57, 198, 61, 194, 65, 190, 69, 186, 73, 182, 77, 178, 81, 174, 85, 170, 89,
    166, 93, 162, 97, 158, 101, 154, 105, 150, 109, 146, 113, 142, 117, 138, 121, 134, 125, 130, 2,
    253, 6, 249, 10, 245, 14, 241, 18, 237, 22, 233, 26, 229, 30, 225, 34, 221, 38, 217, 42, 213,
    46, 209, 50, 205, 54, 201, 58, 197, 62, 193, 66, 189, 70, 185, 74, 181, 78, 177, 82, 173, 86,
    169, 90, 165, 94, 161, 98, 157, 102, 153, 106, 149, 110, 145, 114, 141, 118, 137, 122, 133,
    126, 129,
];
fn cunq_rows() -> Vec<u8> {
    CUNQ.to_vec()
}

fn tables() -> Vec<u8> {
    let cunq = cunq_rows();
    let tab_wunq = 28 * 4usize;
    let tab_cunq = tab_wunq + 144;
    let mut p = vec![0u8; tab_cunq + 936];
    for i in 0..12 {
        p[i * 4..i * 4 + 4]
            .copy_from_slice(&((tab_wunq + WUNQ_OFF[i] as usize) as u32).to_le_bytes());
    }
    for i in 0..16 {
        p[(12 + i) * 4..(12 + i) * 4 + 4]
            .copy_from_slice(&((tab_cunq + CUNQ_OFF[i] as usize) as u32).to_le_bytes());
    }
    p[tab_wunq..tab_wunq + WUNQ.len()].copy_from_slice(&WUNQ);
    let n = cunq.len().min(936);
    p[tab_cunq..tab_cunq + n].copy_from_slice(&cunq[..n]);
    p
}

pub struct Astc {
    set_layout: vk::DescriptorSetLayout,
    layout: vk::PipelineLayout,
    decode: vk::Pipeline,
    /// the mode-6 build of the encoder (made at once)
    bc7: vk::Pipeline,
    tab: crate::vk::Buf,
    tab_uploaded: bool,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Push {
    in_base: u32,
    out_base: u32,
    width: u32,
    height: u32,
    bw: u32,
    bh: u32,
    bx: u32,
    bslice: u32,
    srgb: u32,
}

/// The BC7 encoder writes mode 6 only (the earlier encoder; `NXGFX_BC7_MODE6=1`, tests compare against it).
pub static BC7_MODE6_ONLY: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
fn mode6_only() -> bool {
    static ENV: std::sync::Once = std::sync::Once::new();
    ENV.call_once(|| {
        if std::env::var("NXGFX_BC7_MODE6").as_deref() == Ok("1") {
            BC7_MODE6_ONLY.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    });
    BC7_MODE6_ONLY.load(std::sync::atomic::Ordering::Relaxed)
}

fn words(b: &[u8]) -> Vec<u32> {
    b.chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

fn make_pipeline(g: &Gpu, layout: vk::PipelineLayout, spv: &[u8]) -> Result<vk::Pipeline, String> {
    let dev = g.dev();
    let w = words(spv);
    let m =
        unsafe { dev.create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&w), None) }
            .map_err(|e| e.to_string())?;
    let st = vk::PipelineShaderStageCreateInfo::default()
        .stage(vk::ShaderStageFlags::COMPUTE)
        .module(m)
        .name(c"main");
    let p = unsafe {
        dev.create_compute_pipelines(
            g.pipes.cache,
            &[vk::ComputePipelineCreateInfo::default()
                .stage(st)
                .layout(layout)],
            None,
        )
    }
    .map_err(|(_, e)| e.to_string());
    unsafe { dev.destroy_shader_module(m, None) };
    Ok(p?[0])
}

/// The full BC7 encoder, when its pipeline is ready.
fn bc7_full() -> Option<vk::Pipeline> {
    let r = BC7_FULL.load(std::sync::atomic::Ordering::Acquire);
    (r != 0).then(|| vk::Handle::from_raw(r))
}

/// Waits (up to 60 s) until the full BC7 encoder is ready; makes the ASTC pipelines first if needed. Tests and tools.
pub fn wait_bc7_encoder(g: &Gpu) -> bool {
    {
        let mut a = g.astc.lock().unwrap();
        if a.is_none() {
            match Astc::new(g) {
                Ok(x) => *a = Some(x),
                Err(_) => return false,
            }
        }
    }
    let t = std::time::Instant::now();
    while bc7_full().is_none() && t.elapsed().as_secs() < 60 {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    bc7_full().is_some()
}

impl Astc {
    fn new(g: &Gpu) -> Result<Astc, String> {
        let dev = g.dev();
        let binds: Vec<_> = (0..3)
            .map(|i| {
                vk::DescriptorSetLayoutBinding::default()
                    .binding(i)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::COMPUTE)
            })
            .collect();
        let set_layout = unsafe {
            dev.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&binds),
                None,
            )
        }
        .map_err(|e| e.to_string())?;
        let pcr = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::COMPUTE)
            .size(core::mem::size_of::<Push>() as u32)];
        let sl = [set_layout];
        let layout = unsafe {
            dev.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .set_layouts(&sl)
                    .push_constant_ranges(&pcr),
                None,
            )
        }
        .map_err(|e| e.to_string())?;
        let t0 = std::time::Instant::now();
        let decode = make_pipeline(g, layout, DECODE_SPV)?;
        let t1 = t0.elapsed();
        let bc7 = make_pipeline(g, layout, BC7_M6_SPV)?;
        crate::vklog!(
            "ASTC pipelines made: decode {} ms, BC7 encode (mode 6) {} ms",
            t1.as_millis(),
            (t0.elapsed() - t1).as_millis()
        );
        let raw = vk::Handle::as_raw(layout);
        std::thread::Builder::new()
            .name("bc7 pipeline".into())
            .spawn(move || {
                let Some(g) = Gpu::try_get() else { return };
                let t = std::time::Instant::now();
                match make_pipeline(g, vk::Handle::from_raw(raw), BC7_SPV) {
                    Ok(p) => {
                        crate::vk::pcache::note_created(1);
                        BC7_FULL.store(vk::Handle::as_raw(p), std::sync::atomic::Ordering::Release);
                        crate::vklog!(
                            "BC7 encoder (modes 1, 3, 5, 6, 7) ready in {} ms",
                            t.elapsed().as_millis()
                        );
                    }
                    Err(e) => crate::log::red(&format!("BC7 encoder pipeline: {e} (mode 6 only)")),
                }
            })
            .map_err(|e| e.to_string())?;
        let tab = g.buffer(
            tables().len() as u64,
            vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_DST,
            MemoryLocation::GpuOnly,
            "astc tables",
        )?;
        Ok(Astc {
            set_layout,
            layout,
            decode,
            bc7,
            tab,
            tab_uploaded: false,
        })
    }
}

fn barrier(g: &Gpu, cb: vk::CommandBuffer) {
    crate::vk::queue::full_barrier(g, cb);
}

/// Whether a level of `bc_bytes` BC7 bytes goes through the disk cache (`astc_cache`).
fn cache_applies(bc7: bool, bc_bytes: u64, full: bool) -> bool {
    bc7 && bc_bytes as usize >= crate::tex::astc_cache::MIN_BYTES
        && full
        && crate::tex::astc_cache::active()
}

/// A level's disk-cache key and, on a hit, its BC7 blocks, looked up ahead of `record` (MKW-SPIKE: with the images'
/// lock released, as the key hashes every block and a hit reads the level from the pack file).
pub struct CachePre {
    key: crate::tex::astc_cache::Key,
    hit: Option<Vec<u8>>,
}

/// `record`'s cache lookup for one level, made ahead; None when the level does not go through the cache.
#[allow(clippy::too_many_arguments)]
pub fn cache_probe(
    blocks: &[u8],
    w: u32,
    h: u32,
    slices: u32,
    bw: u32,
    bh: u32,
    srgb: bool,
    bc7: bool,
) -> Option<CachePre> {
    let bc_bytes = w.div_ceil(4) as u64 * h.div_ceil(4) as u64 * slices as u64 * 16;
    let full = !mode6_only() && bc7_full().is_some();
    if !(4..=12).contains(&bw) || !(4..=12).contains(&bh) || !cache_applies(bc7, bc_bytes, full) {
        return None;
    }
    let key = crate::tex::astc_cache::key(blocks, w, h, slices, bw, bh, srgb);
    let hit = crate::tex::astc_cache::get(key, bc_bytes as usize);
    Some(CachePre { key, hit })
}

/// Records the decode of `blocks` (one level and layer, block rows of a w x h x slices texture, 16 bytes a block)
/// into `image` at `level` / `layer`, as RGBA8 or, with `bc7`, BC7. Into the submission's upload command buffer.
/// `pre`: the level's disk-cache lookup made ahead (`cache_probe`), or None to make it here.
#[allow(clippy::too_many_arguments)]
pub fn record(
    g: &Gpu,
    sub: &mut Sub,
    blocks: &[u8],
    w: u32,
    h: u32,
    slices: u32,
    bw: u32,
    bh: u32,
    srgb: bool,
    bc7: bool,
    image: vk::Image,
    level: u32,
    layer: u32,
    pre: Option<CachePre>,
) {
    let Some((out, off, rows, bufrow)) =
        dispatch(g, sub, blocks, w, h, slices, bw, bh, srgb, bc7, pre)
    else {
        return;
    };
    let (ex, ey) = (w.div_ceil(4), h.div_ceil(4));
    let r = vk::BufferImageCopy {
        buffer_offset: off,
        buffer_row_length: bufrow,
        buffer_image_height: rows,
        image_subresource: vk::ImageSubresourceLayers {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            mip_level: level,
            base_array_layer: layer,
            layer_count: 1,
        },
        image_offset: vk::Offset3D::default(),
        image_extent: vk::Extent3D {
            width: w,
            height: h,
            depth: slices,
        },
    };
    let _ = (ex, ey);
    unsafe {
        g.dev()
            .cmd_copy_buffer_to_image(sub.pre, out, image, vk::ImageLayout::GENERAL, &[r])
    };
}

/// Encodes `blocks` (one w x h region of an ASTC level, block rows, 16 bytes a block) to BC7 in the submission's upload
/// command buffer, for a copy the caller records itself (MKW-ATLAS: a copy from an ASTC texture kept as RGBA8 into an
/// image re-encoded as BC7). Returns the buffer, the BC7 blocks' offset in it, its rows per image and row length in
/// texels (multiples of 4).
#[allow(clippy::too_many_arguments)]
pub fn encode_bc7(
    g: &Gpu,
    sub: &mut Sub,
    blocks: &[u8],
    w: u32,
    h: u32,
    bw: u32,
    bh: u32,
    srgb: bool,
) -> Option<(vk::Buffer, u64, u32, u32)> {
    dispatch(g, sub, blocks, w, h, 1, bw, bh, srgb, true, None)
}

/// The dispatches: blocks -> RGBA8 texels (-> BC7 blocks). Returns the output buffer, the output's offset in it, its
/// rows per image and row length in texels.
#[allow(clippy::too_many_arguments)]
fn dispatch(
    g: &Gpu,
    sub: &mut Sub,
    blocks: &[u8],
    w: u32,
    h: u32,
    slices: u32,
    bw: u32,
    bh: u32,
    srgb: bool,
    bc7: bool,
    pre: Option<CachePre>,
) -> Option<(vk::Buffer, u64, u32, u32)> {
    if !(4..=12).contains(&bw) || !(4..=12).contains(&bh) {
        crate::log::red_once(&format!("ASTC block {bw}x{bh} is not 2D LDR"));
        return None;
    }
    let (ex, ey) = (w.div_ceil(4), h.div_ceil(4));
    let bc_bytes = if bc7 {
        ex as u64 * ey as u64 * slices as u64 * 16
    } else {
        0
    };
    // the disk cache: a level encoded before (this run or an earlier one) is uploaded as it was encoded
    let full = if mode6_only() { None } else { bc7_full() };
    // (MKW-SPIKE) the key and the read may have been made before, without the images' lock (`cache_probe`)
    let (ckey, hit) = match pre {
        Some(p) => (Some(p.key), p.hit),
        None => {
            let ckey = cache_applies(bc7, bc_bytes, full.is_some())
                .then(|| crate::tex::astc_cache::key(blocks, w, h, slices, bw, bh, srgb));
            let hit = ckey.and_then(|k| crate::tex::astc_cache::get(k, bc_bytes as usize));
            (ckey, hit)
        }
    };
    if let Some(b) = hit {
        let (sbuf, soff) = sub.stage_bytes_fresh(g, &b);
        return Some((sbuf, soff, ey * 4, ex * 4));
    }
    let mut a = g.astc.lock().unwrap();
    if a.is_none() {
        match Astc::new(g) {
            Ok(x) => *a = Some(x),
            Err(e) => {
                crate::log::red(&format!("ASTC pipelines: {e}"));
                return None;
            }
        }
    }
    let a = a.as_mut().unwrap();
    let dev = g.dev();
    if !a.tab_uploaded {
        let t = tables();
        sub.upload(g, &t, a.tab.buf, 0);
        barrier(g, sub.pre);
        a.tab_uploaded = true;
    }
    let (bx, by) = (w.div_ceil(bw), h.div_ceil(bh));
    let texels = w as u64 * h as u64 * slices as u64;
    let (scratch_buf, base) = sub.gpu_scratch(g, texels * 4 + bc_bytes + 256)?;
    let out_texels_off = base;
    let bc_off = base + ((texels * 4 + 255) & !255);
    let (sbuf, soff) = sub.stage_bytes(g, blocks);
    let set = sub.alloc_set(g, a.set_layout);
    let bi = [
        vk::DescriptorBufferInfo {
            buffer: sbuf,
            offset: soff,
            range: blocks.len().max(16) as u64,
        },
        vk::DescriptorBufferInfo {
            buffer: scratch_buf,
            offset: out_texels_off,
            range: texels * 4,
        },
        vk::DescriptorBufferInfo {
            buffer: a.tab.buf,
            offset: 0,
            range: vk::WHOLE_SIZE,
        },
    ];
    let wr: Vec<_> = (0..3)
        .map(|i| {
            vk::WriteDescriptorSet::default()
                .dst_set(set)
                .dst_binding(i as u32)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .buffer_info(&bi[i..i + 1])
        })
        .collect();
    crate::vk::lost::checkpoint(g, sub.pre, false, || {
        format!("an ASTC decode {w}x{h}x{slices} ({bw}x{bh} blocks)")
    });
    unsafe {
        dev.update_descriptor_sets(&wr, &[]);
        dev.cmd_bind_pipeline(sub.pre, vk::PipelineBindPoint::COMPUTE, a.decode);
        dev.cmd_bind_descriptor_sets(
            sub.pre,
            vk::PipelineBindPoint::COMPUTE,
            a.layout,
            0,
            &[set],
            &[],
        );
        let pc = Push {
            in_base: 0,
            out_base: 0,
            width: w,
            height: h,
            bw,
            bh,
            bx,
            bslice: bx * by,
            srgb: srgb as u32,
        };
        dev.cmd_push_constants(
            sub.pre,
            a.layout,
            vk::ShaderStageFlags::COMPUTE,
            0,
            core::slice::from_raw_parts(
                &pc as *const Push as *const u8,
                core::mem::size_of::<Push>(),
            ),
        );
        let bpw = if bw * bh <= 64 { 64 / (bw * bh) } else { 1 };
        dev.cmd_dispatch(sub.pre, bx.div_ceil(bpw), by, slices);
    }
    barrier(g, sub.pre);
    let result = if bc7 {
        let set2 = sub.alloc_set(g, a.set_layout);
        let bi2 = [
            vk::DescriptorBufferInfo {
                buffer: scratch_buf,
                offset: out_texels_off,
                range: texels * 4,
            },
            vk::DescriptorBufferInfo {
                buffer: scratch_buf,
                offset: bc_off,
                range: bc_bytes,
            },
        ];
        let wr2: Vec<_> = (0..2)
            .map(|i| {
                vk::WriteDescriptorSet::default()
                    .dst_set(set2)
                    .dst_binding(i as u32)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(&bi2[i..i + 1])
            })
            .collect();
        unsafe {
            dev.update_descriptor_sets(&wr2, &[]);
            dev.cmd_bind_pipeline(
                sub.pre,
                vk::PipelineBindPoint::COMPUTE,
                full.unwrap_or(a.bc7),
            );
            dev.cmd_bind_descriptor_sets(
                sub.pre,
                vk::PipelineBindPoint::COMPUTE,
                a.layout,
                0,
                &[set2],
                &[],
            );
            let pc = Push {
                in_base: 0,
                out_base: 0,
                width: w,
                height: h,
                bw: ex,
                bh: ex * ey,
                bx: mode6_only() as u32,
                bslice: 0,
                srgb: 0,
            };
            dev.cmd_push_constants(
                sub.pre,
                a.layout,
                vk::ShaderStageFlags::COMPUTE,
                0,
                core::slice::from_raw_parts(
                    &pc as *const Push as *const u8,
                    core::mem::size_of::<Push>(),
                ),
            );
            dev.cmd_dispatch(sub.pre, ex.div_ceil(8), ey.div_ceil(8), slices);
        }
        barrier(g, sub.pre);
        // a miss goes into the disk cache once the submission has run
        if let Some(k) = ckey {
            if let Ok(rb) = g.buffer(
                bc_bytes,
                vk::BufferUsageFlags::TRANSFER_DST,
                MemoryLocation::GpuToCpu,
                "astc cache readback",
            ) {
                unsafe {
                    dev.cmd_copy_buffer(
                        sub.pre,
                        scratch_buf,
                        rb.buf,
                        &[vk::BufferCopy {
                            src_offset: bc_off,
                            dst_offset: 0,
                            size: bc_bytes,
                        }],
                    )
                };
                sub.post.push(crate::vk::queue::Post::Callback(Box::new(
                    move |g: &Gpu| {
                        let v = unsafe { core::slice::from_raw_parts(rb.ptr, bc_bytes as usize) }
                            .to_vec();
                        g.free_buffer(rb);
                        crate::tex::astc_cache::put(k, v);
                    },
                )));
                barrier(g, sub.pre);
            }
        }
        // the copy reads the BC7 blocks where they are (256-aligned)
        (scratch_buf, bc_off, ey * 4, ex * 4)
    } else {
        (scratch_buf, out_texels_off, h, w)
    };
    Some(result)
}

/// Decodes ASTC blocks on the GPU and reads the texels back (tests and tools): RGBA8, or with `bc7` the BC7 blocks.
pub fn decode_blocks(
    g: &'static Gpu,
    blocks: &[u8],
    w: u32,
    h: u32,
    bw: u32,
    bh: u32,
    srgb: bool,
    bc7: bool,
) -> Option<Vec<u8>> {
    let size = if bc7 {
        w.div_ceil(4) as u64 * h.div_ceil(4) as u64 * 16
    } else {
        w as u64 * h as u64 * 4
    };
    let rb = g
        .buffer(
            size,
            vk::BufferUsageFlags::TRANSFER_DST,
            MemoryLocation::GpuToCpu,
            "astc readback",
        )
        .ok()?;
    let mut ok = false;
    let f = crate::vk::queue::oneoff(g, |g, sub, _imgs| {
        if let Some((buf, off, _, _)) = dispatch(g, sub, blocks, w, h, 1, bw, bh, srgb, bc7, None) {
            unsafe {
                g.dev().cmd_copy_buffer(
                    sub.pre,
                    buf,
                    rb.buf,
                    &[vk::BufferCopy {
                        src_offset: off,
                        dst_offset: 0,
                        size,
                    }],
                )
            };
            ok = true;
        }
    });
    crate::vk::queue::wait_fence(f, u64::MAX);
    let out = ok.then(|| unsafe { core::slice::from_raw_parts(rb.ptr, size as usize) }.to_vec());
    g.free_buffer(rb);
    out
}

/// Decodes BC7 blocks with the GPU's own BC7 decoder (tests: an independent check of the encoder's bit layouts): the
/// blocks into a BC7 image, blitted into an RGBA8 one, read back. None when the device cannot blit from BC7.
pub fn bc7_hw_decode(g: &'static Gpu, blocks: &[u8], w: u32, h: u32) -> Option<Vec<u8>> {
    use gpu_allocator::vulkan::{AllocationCreateDesc, AllocationScheme};
    if !g
        .d
        .format_features(vk::Format::BC7_UNORM_BLOCK)
        .contains(vk::FormatFeatureFlags::BLIT_SRC)
    {
        return None;
    }
    let dev = g.dev();
    let mk = |f: vk::Format| -> Option<(vk::Image, gpu_allocator::vulkan::Allocation)> {
        let ci = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(f)
            .extent(vk::Extent3D {
                width: w,
                height: h,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .usage(
                vk::ImageUsageFlags::TRANSFER_DST
                    | vk::ImageUsageFlags::TRANSFER_SRC
                    | vk::ImageUsageFlags::SAMPLED,
            );
        let im = unsafe { dev.create_image(&ci, None) }.ok()?;
        let req = unsafe { dev.get_image_memory_requirements(im) };
        let a = g
            .alloc
            .lock()
            .unwrap()
            .allocate(&AllocationCreateDesc {
                name: "bc7 check",
                requirements: req,
                location: MemoryLocation::GpuOnly,
                linear: false,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
            })
            .ok()?;
        unsafe { dev.bind_image_memory(im, a.memory(), a.offset()) }.ok()?;
        Some((im, a))
    };
    let (src, sa) = mk(vk::Format::BC7_UNORM_BLOCK)?;
    let (dst, da) = mk(vk::Format::R8G8B8A8_UNORM)?;
    let size = w as u64 * h as u64 * 4;
    let rb = g
        .buffer(
            size,
            vk::BufferUsageFlags::TRANSFER_DST,
            MemoryLocation::GpuToCpu,
            "bc7 check readback",
        )
        .ok()?;
    let sr = vk::ImageSubresourceRange {
        aspect_mask: vk::ImageAspectFlags::COLOR,
        base_mip_level: 0,
        level_count: 1,
        base_array_layer: 0,
        layer_count: 1,
    };
    let sl = vk::ImageSubresourceLayers {
        aspect_mask: vk::ImageAspectFlags::COLOR,
        mip_level: 0,
        base_array_layer: 0,
        layer_count: 1,
    };
    let f = crate::vk::queue::oneoff(g, |g, sub, _| {
        let cb = sub.pre;
        let (sbuf, soff) = sub.stage_bytes_fresh(g, blocks);
        let to_general = |im: vk::Image| {
            vk::ImageMemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::empty())
                .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE | vk::AccessFlags::TRANSFER_READ)
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::GENERAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(im)
                .subresource_range(sr)
        };
        unsafe {
            dev.cmd_pipeline_barrier(
                cb,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[to_general(src), to_general(dst)],
            );
            let r = vk::BufferImageCopy {
                buffer_offset: soff,
                buffer_row_length: 0,
                buffer_image_height: 0,
                image_subresource: sl,
                image_offset: vk::Offset3D::default(),
                image_extent: vk::Extent3D {
                    width: w,
                    height: h,
                    depth: 1,
                },
            };
            dev.cmd_copy_buffer_to_image(cb, sbuf, src, vk::ImageLayout::GENERAL, &[r]);
        }
        barrier(g, cb);
        let off = [
            vk::Offset3D::default(),
            vk::Offset3D {
                x: w as i32,
                y: h as i32,
                z: 1,
            },
        ];
        let blit = vk::ImageBlit {
            src_subresource: sl,
            src_offsets: off,
            dst_subresource: sl,
            dst_offsets: off,
        };
        unsafe {
            dev.cmd_blit_image(
                cb,
                src,
                vk::ImageLayout::GENERAL,
                dst,
                vk::ImageLayout::GENERAL,
                &[blit],
                vk::Filter::NEAREST,
            )
        };
        barrier(g, cb);
        let r = vk::BufferImageCopy {
            buffer_offset: 0,
            buffer_row_length: 0,
            buffer_image_height: 0,
            image_subresource: sl,
            image_offset: vk::Offset3D::default(),
            image_extent: vk::Extent3D {
                width: w,
                height: h,
                depth: 1,
            },
        };
        unsafe { dev.cmd_copy_image_to_buffer(cb, dst, vk::ImageLayout::GENERAL, rb.buf, &[r]) };
    });
    crate::vk::queue::wait_fence(f, u64::MAX);
    let out = unsafe { core::slice::from_raw_parts(rb.ptr, size as usize) }.to_vec();
    g.free_buffer(rb);
    unsafe {
        dev.destroy_image(src, None);
        dev.destroy_image(dst, None);
    }
    let mut al = g.alloc.lock().unwrap();
    let _ = al.free(sa);
    let _ = al.free(da);
    Some(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn colour_tables_match_the_reference_rows() {
        // The rows the reference implementation ships for QUANT_6 and QUANT_12 (spot check of the generator).
        let c = super::cunq_rows();
        assert_eq!(&c[..6], &[0, 255, 51, 204, 102, 153]);
        assert_eq!(&c[6..14], &[0, 36, 73, 109, 146, 182, 219, 255]);
        assert_eq!(&c[14..24], &[0, 255, 28, 227, 56, 199, 84, 171, 113, 142]);
        assert_eq!(
            &c[24..36],
            &[0, 255, 69, 186, 23, 232, 92, 163, 46, 209, 116, 139]
        );
        assert_eq!(c.len(), 936);
    }
}
