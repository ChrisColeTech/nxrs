//! Sparse textures (`NVN_TEXTURE_FLAGS_SPARSE` in a GPU-only sparse virtual pool) as Vulkan sparse images, ported
//! from mkw-recomp's `vk_sparse.c` (MIT).
//!
//! The game maps 64 KiB pages of a texture's storage with nvnMemoryPoolMapVirtual; each mapped page binds the Vulkan
//! sparse blocks that hold the same texels to memory of that physical page. Storage layout: a layer is its levels
//! with whole NVN tiles (`tex::layout::sparse_tiles`), level after level, each in row-major tiles of one page; the
//! layer stride is those pages. The levels outside the tiles (smaller than a tile, and the Vulkan mip tail) are
//! never mapped by Mario Kart World but are copied into: they get memory of their own, bound once.
//!
//! Memory: one slot (the image's block size, 64 KiB) per (physical page, Vulkan block inside it, format), so virtual
//! pages mapped to one physical page share their texels, as on the console. Slots come zeroed from blocks of 256
//! and are freed once no binding names them and the GPU is past the unbind. Without it every 8192x8192x68 terrain
//! array was a whole image (1.4 to 5.8 GB each): Mario Kart World made 19 GB of them in its first second.
//!
//! Changed blocks are listed per image and bound before the next submission (`flush`, under the queue lock):
//! vkQueueBindSparse waits for every earlier submission and signals the sparse timeline every later submission
//! waits on. `NXGFX_SPARSE=0`: sparse textures are whole images (as before). `NXGFX_SPARSE_MIB`: slot memory cap
//! (default half the largest device-local heap).

use super::Gpu;
use ash::vk;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};
use gpu_allocator::MemoryLocation;
use std::collections::HashMap;
use std::sync::Mutex;

const PAGE: u64 = 0x10000;
const BLOCK_SLOTS: usize = 256;

struct Block {
    buf: vk::Buffer,
    /// kept for the life of the block
    _alloc: Option<Allocation>,
    mem: vk::DeviceMemory,
    base: u64,
    slot_size: u64,
    type_bits: u32,
    used: usize,
    busy: Vec<u8>, // 0 never used (zero), 1 in use, 2 freed (texels stay)
    fresh: bool,   // not zero-filled yet
}

#[derive(Hash, PartialEq, Eq, Clone, Copy)]
struct SlotKey {
    pa: u64,
    sub: u32,
    fmt: i32,
}

struct Slot {
    blk: usize,
    idx: usize,
    refs: i32,
}

struct Img {
    image: vk::Image,
    addr: u64,
    size: u64,
    fmt: vk::Format,
    aspect: vk::ImageAspectFlags,
    w: u32,
    h: u32,
    layers: u32,
    nlev: u32,
    ntw: u32,
    nth: u32,
    gw: u32,
    gh: u32,
    nnvn: u32,
    tiles_x: [u32; 16],
    tiles_y: [u32; 16],
    tile0: [u32; 16],
    stride: u32,
    bx: [u32; 16],
    blk0: [u32; 16],
    nblk: u32,
    slot_size: u64,
    type_bits: u32,
    bound: Vec<Option<SlotKey>>,
    dirty: Vec<bool>,
    dl: Vec<u32>,
    // levels outside the tiles and the mip tail: memory of their own, shared by the images over the same storage with
    // the same layout (`State::tails`)
    tail: Option<(TailKey, vk::DeviceMemory, u64)>,
    tail_pending: bool,
    tail_end: u32,
    nmips: u32,
    tail_blocks: u32,
    tail_req: Option<vk::SparseImageMemoryRequirements>,
}

/// The storage and layout an image's levels outside the tiles have: (storage address, storage size, Vulkan format,
/// width, height, layers, levels, first level outside the tiles, slot size, memory types).
type TailKey = (u64, u64, i32, u32, u32, u32, u32, u32, u64, u32);

#[derive(Default)]
struct State {
    imgs: Vec<Img>,
    /// The memory of the levels outside the tiles, by storage and layout, and the images binding it (MKW-ATLAS). Two
    /// texture objects over one storage are one texture on the console: Mario Kart World's terrain arrays come in
    /// such pairs (8192x8192x68 BC7, one storage), it copies the small levels (5-11) through one and samples them
    /// through the other. The tiles were shared already (one slot per physical page); these levels now are too.
    tails: HashMap<TailKey, (Allocation, u32)>,
    slots: HashMap<SlotKey, Slot>,
    blocks: Vec<Block>,
    dying: Vec<SlotKey>,
    dying_tails: Vec<Allocation>,
    n_slots: u64,
    n_over: u64,
    n_failed: u64,
    n_binds: u64,
    n_calls: u64,
    timeline: vk::Semaphore,
    value: u64,
}
unsafe impl Send for State {}

static ST: Mutex<Option<State>> = Mutex::new(None);

/// Granules of sparse memory-pool buffers (`mem.rs`) bound before the next submission, with the sparse textures'.
static BUF_BINDS: Mutex<Vec<(vk::Buffer, vk::SparseMemoryBind)>> = Mutex::new(Vec::new());

/// A granule of a sparse pool buffer, bound by the next `flush` (before the submission being recorded runs).
pub fn bind_buffer(buf: vk::Buffer, b: vk::SparseMemoryBind) {
    BUF_BINDS.lock().unwrap().push((buf, b));
}

fn on() -> bool {
    std::env::var("NXGFX_SPARSE").as_deref() != Ok("0")
}

fn budget(g: &Gpu) -> u64 {
    static V: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        let mib = std::env::var("NXGFX_SPARSE_MIB")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or_else(|| {
                let mp = &g.d.mem_props;
                let vram = (0..mp.memory_heap_count as usize)
                    .filter(|&h| {
                        mp.memory_heaps[h]
                            .flags
                            .contains(vk::MemoryHeapFlags::DEVICE_LOCAL)
                    })
                    .map(|h| mp.memory_heaps[h].size)
                    .max()
                    .unwrap_or(0);
                (vram >> 20) / 2
            });
        crate::vklog!("sparse textures: tile memory up to {mib} MB (NXGFX_SPARSE_MIB; 0 lifts it)");
        if mib == 0 {
            u64::MAX
        } else {
            mib << 20
        }
    })
}

/// Whether a texture becomes a sparse image: a sparse texture in a GPU-only sparse virtual pool, 2D (array),
/// single-sampled, sampled.
pub fn wanted(g: &Gpu, t: &crate::nvn::obj::tex_t, ci: &vk::ImageCreateInfo) -> bool {
    on() && g.d.caps.sparse
        && t.d.flags as u32 & crate::nvn::types::tex_flags::SPARSE != 0
        && crate::nvn::vpool::is_sparse(t.mem as u64)
        && crate::nvn::vpool::gpu_only(t.mem as u64)
        && ci.image_type == vk::ImageType::TYPE_2D
        && ci.samples == vk::SampleCountFlags::TYPE_1
        && ci.usage.contains(vk::ImageUsageFlags::SAMPLED)
}

/// Converts a driver's sparse block size given in compressed blocks into texels (mkw-recomp's rule: the standard
/// block is 64 KiB).
fn granularity_texels(f: vk::Format, gw: &mut u32, gh: &mut u32) {
    let raw = f.as_raw();
    let (bw, bh, bpb): (u32, u32, u64) = if (vk::Format::BC1_RGB_UNORM_BLOCK.as_raw()
        ..=vk::Format::BC7_SRGB_BLOCK.as_raw())
        .contains(&raw)
    {
        let half = raw <= vk::Format::BC1_RGBA_SRGB_BLOCK.as_raw()
            || f == vk::Format::BC4_UNORM_BLOCK
            || f == vk::Format::BC4_SNORM_BLOCK;
        (4, 4, if half { 8 } else { 16 })
    } else {
        return;
    };
    if (*gw / bw) as u64 * (*gh / bh) as u64 * bpb == 65536
        || *gw as u64 * *gh as u64 * bpb != 65536
    {
        return;
    }
    *gw *= bw;
    *gh *= bh;
}

/// Creates the sparse image of a texture (None: it cannot be one, the caller makes an ordinary image).
pub fn create(g: &Gpu, t: &crate::nvn::obj::tex_t, ci0: &vk::ImageCreateInfo) -> Option<vk::Image> {
    let mut ci = *ci0;
    if !matches!(
        t.d.target,
        crate::nvn::types::target::CUBE | crate::nvn::types::target::CUBE_ARRAY
    ) {
        ci.flags &= !vk::ImageCreateFlags::CUBE_COMPATIBLE;
    }
    ci.flags |= vk::ImageCreateFlags::SPARSE_BINDING
        | vk::ImageCreateFlags::SPARSE_RESIDENCY
        | vk::ImageCreateFlags::SPARSE_ALIASED;
    let inst = &g.d.instance;
    let fp = unsafe {
        inst.get_physical_device_sparse_image_format_properties(
            g.d.pdev,
            ci.format,
            ci.image_type,
            ci.samples,
            ci.usage,
            ci.tiling,
        )
    };
    let Some(fp0) = fp.first().copied() else {
        crate::log::red_once(&format!(
            "sparse textures: {:?} usage {:?} has no sparse blocks: a whole image",
            ci.format, ci.usage
        ));
        return None;
    };
    if fp0
        .flags
        .contains(vk::SparseImageFormatFlags::NONSTANDARD_BLOCK_SIZE)
    {
        crate::log::red_once(&format!(
            "sparse textures: {:?} has no standard sparse blocks: a whole image",
            ci.format
        ));
        return None;
    }
    let (ntl, ntw, nth, _) = crate::tex::layout::sparse_tiles(&t.d);
    let (mut gw, mut gh) = (fp0.image_granularity.width, fp0.image_granularity.height);
    granularity_texels(ci.format, &mut gw, &mut gh);
    if gw == 0 || gh == 0 || ntw % gw != 0 || nth % gh != 0 || !(0..=16).contains(&ntl) {
        crate::log::red_once(&format!("sparse textures: NVN tile {ntw}x{nth} is not whole Vulkan blocks of {gw}x{gh} ({:?}): a whole image", ci.format));
        return None;
    }
    let dev = g.dev();
    let image = match unsafe { dev.create_image(&ci, None) } {
        Ok(i) => i,
        Err(e) => {
            crate::log::red_once(&format!(
                "sparse textures: vkCreateImage {}x{} layers {}: {e}: a whole image",
                ci.extent.width, ci.extent.height, ci.array_layers
            ));
            return None;
        }
    };
    let mr = unsafe { dev.get_image_memory_requirements(image) };
    let sr = unsafe { dev.get_image_sparse_memory_requirements(image) };
    let sr0 = sr.first().copied();
    let mut s = Img {
        image,
        addr: t.mem as u64,
        size: t.l.size,
        fmt: ci.format,
        aspect: fp0.aspect_mask,
        w: ci.extent.width,
        h: ci.extent.height,
        layers: ci.array_layers,
        nlev: (ntl as u32).min(ci.mip_levels),
        ntw,
        nth,
        gw,
        gh,
        nnvn: ntl as u32,
        tiles_x: [0; 16],
        tiles_y: [0; 16],
        tile0: [0; 16],
        stride: 0,
        bx: [0; 16],
        blk0: [0; 16],
        nblk: 0,
        slot_size: mr.alignment,
        type_bits: mr.memory_type_bits,
        bound: Vec::new(),
        dirty: Vec::new(),
        dl: Vec::new(),
        tail: None,
        tail_pending: false,
        tail_end: ci.mip_levels,
        nmips: ci.mip_levels,
        tail_blocks: 0,
        tail_req: sr0,
    };
    if let Some(r) = sr0 {
        s.nlev = s.nlev.min(r.image_mip_tail_first_lod);
    }
    let (mut page, mut blk) = (0u32, 0u32);
    for l in 0..s.nnvn as usize {
        let (lw, lh) = ((s.w >> l).max(1), (s.h >> l).max(1));
        s.tiles_x[l] = lw.div_ceil(s.ntw);
        s.tiles_y[l] = lh.div_ceil(s.nth);
        s.tile0[l] = page;
        page += s.tiles_x[l] * s.tiles_y[l];
        s.stride = page;
        if (l as u32) < s.nlev {
            s.bx[l] = lw.div_ceil(s.gw);
            s.blk0[l] = blk;
            blk += s.bx[l] * lh.div_ceil(s.gh);
            s.nblk = blk;
        }
    }
    let n = (s.layers * s.nblk) as usize;
    s.bound = vec![None; n];
    s.dirty = vec![false; n];
    let mut lk = ST.lock().unwrap();
    let st = lk.get_or_insert_with(Default::default);
    tail_alloc(g, st, &mut s);
    // coarse levels first: a full budget leaves fine detail out rather than whole layers
    for l in (0..s.nlev as usize).rev() {
        for y in 0..s.layers {
            let lo = s.addr + (y as u64 * s.stride as u64 + s.tile0[l] as u64) * PAGE;
            let hi = lo + (s.tiles_x[l] * s.tiles_y[l]) as u64 * PAGE;
            bind_range(g, st, &mut s, lo, hi);
        }
    }
    crate::vklog!(
        "sparse image {}: {:#x} {}x{} layers {} {:?}, NVN tile {}x{} ({} levels, {} pages a layer), Vulkan block {}x{} of {} bytes, {} MB of address space; {} slots live",
        st.imgs.len() + 1,
        s.addr,
        s.w,
        s.h,
        s.layers,
        s.fmt,
        s.ntw,
        s.nth,
        ntl,
        s.stride,
        s.gw,
        s.gh,
        s.slot_size,
        mr.size >> 20,
        st.n_slots
    );
    crate::vklog!(
        "sparse image {}: ser{} storage {:#x} ({} pages; layers x pages {}), levels {}, Vulkan mip tail from {:?}",
        st.imgs.len() + 1,
        t.serial,
        s.size,
        s.size / PAGE,
        s.layers as u64 * s.stride as u64,
        s.nmips,
        s.tail_req.map(|r| (
            r.image_mip_tail_first_lod,
            r.image_mip_tail_size,
            r.image_mip_tail_stride
        ))
    );
    st.imgs.push(s);
    Some(image)
}

fn alloc_mem(g: &Gpu, size: u64, align: u64, type_bits: u32, name: &str) -> Option<Allocation> {
    let req = vk::MemoryRequirements {
        size,
        alignment: align,
        memory_type_bits: type_bits,
    };
    g.alloc
        .lock()
        .unwrap()
        .allocate(&AllocationCreateDesc {
            name,
            requirements: req,
            location: MemoryLocation::GpuOnly,
            linear: false,
            allocation_scheme: AllocationScheme::GpuAllocatorManaged,
        })
        .ok()
}

fn tail_count(s: &Img) -> u32 {
    match s.tail_req {
        Some(r) if s.tail_end < s.nmips && r.image_mip_tail_size != 0 => {
            if r.format_properties
                .flags
                .contains(vk::SparseImageFormatFlags::SINGLE_MIPTAIL)
            {
                1
            } else {
                s.layers
            }
        }
        _ => 0,
    }
}

fn round_up(v: u64, a: u64) -> u64 {
    v.div_ceil(a) * a
}

/// The key under which images share their levels outside the tiles (`State::tails`).
fn tail_key(s: &Img) -> TailKey {
    (
        s.addr,
        s.size,
        s.fmt.as_raw(),
        s.w,
        s.h,
        s.layers,
        s.nmips,
        s.nlev,
        s.slot_size,
        s.type_bits,
    )
}

/// Memory for the levels outside NVN's tiles and the Vulkan mip tail: an image over the same storage with the same
/// layout lends its own.
fn tail_alloc(g: &Gpu, st: &mut State, s: &mut Img) {
    s.tail_end = s
        .tail_req
        .map(|r| r.image_mip_tail_first_lod.min(s.nmips))
        .unwrap_or(s.nmips);
    for l in s.nlev..s.tail_end {
        let (lw, lh) = ((s.w >> l).max(1), (s.h >> l).max(1));
        s.tail_blocks += lw.div_ceil(s.gw) * lh.div_ceil(s.gh);
    }
    let bytes = s.layers as u64 * s.tail_blocks as u64 * s.slot_size
        + tail_count(s) as u64
            * round_up(
                s.tail_req.map(|r| r.image_mip_tail_size).unwrap_or(0),
                s.slot_size,
            );
    if bytes == 0 {
        return;
    }
    let key = tail_key(s);
    if let Some((a, refs)) = st.tails.get_mut(&key) {
        *refs += 1;
        let (m, o) = unsafe { (a.memory(), a.offset()) };
        s.tail = Some((key, m, o));
        s.tail_pending = true;
        crate::vklog!(
            "sparse textures: {}x{} layers {} {:?}: the levels outside the tiles share the memory of the image over the same storage {:#x} ({} images)",
            s.w, s.h, s.layers, s.fmt, s.addr, refs
        );
        return;
    }
    match alloc_mem(g, bytes, s.slot_size, s.type_bits, "sparse tail") {
        Some(a) => {
            let (m, o) = unsafe { (a.memory(), a.offset()) };
            st.tails.insert(key, (a, 1));
            s.tail = Some((key, m, o));
            s.tail_pending = true;
        }
        None => crate::log::red(&format!("sparse textures: no memory for the {} KB of levels outside the tiles ({}x{} layers {})", bytes >> 10, s.w, s.h, s.layers)),
    }
}

fn new_block(g: &Gpu, st: &mut State, type_bits: u32, slot_size: u64) -> Option<usize> {
    let dev = g.dev();
    let size = slot_size * BLOCK_SLOTS as u64;
    let bi = vk::BufferCreateInfo::default()
        .size(size)
        .usage(vk::BufferUsageFlags::TRANSFER_DST | vk::BufferUsageFlags::TRANSFER_SRC);
    let buf = unsafe { dev.create_buffer(&bi, None) }.ok()?;
    let rb = unsafe { dev.get_buffer_memory_requirements(buf) };
    let both = rb.memory_type_bits & type_bits;
    let (types, buf) = if both != 0 {
        (both, buf)
    } else {
        unsafe { dev.destroy_buffer(buf, None) };
        (type_bits, vk::Buffer::null())
    };
    let Some(a) = alloc_mem(g, size, slot_size.max(rb.alignment), types, "sparse tiles") else {
        if buf != vk::Buffer::null() {
            unsafe { dev.destroy_buffer(buf, None) };
        }
        return None;
    };
    let (mem, base) = unsafe { (a.memory(), a.offset()) };
    if buf != vk::Buffer::null() {
        unsafe { dev.bind_buffer_memory(buf, mem, base) }.ok()?;
    }
    st.blocks.push(Block {
        buf,
        _alloc: Some(a),
        mem,
        base,
        slot_size,
        type_bits,
        used: 0,
        busy: vec![0; BLOCK_SLOTS],
        fresh: true,
    });
    Some(st.blocks.len() - 1)
}

fn slot_get(g: &Gpu, st: &mut State, s: &Img, pa: u64, sub: u32) -> Option<SlotKey> {
    let k = SlotKey {
        pa,
        sub,
        fmt: s.fmt.as_raw(),
    };
    if st.slots.contains_key(&k) {
        return Some(k);
    }
    if s.size > 4 << 20 && (st.n_slots + 1) * s.slot_size > budget(g) {
        st.n_over += 1;
        if st.n_over <= 4 || st.n_over.is_power_of_two() {
            crate::vklog!(
                "sparse textures: tile memory over NXGFX_SPARSE_MIB ({} tiles left non-resident)",
                st.n_over
            );
        }
        return None;
    }
    let mut b = st.blocks.iter().position(|b| {
        b.used < BLOCK_SLOTS && b.type_bits == s.type_bits && b.slot_size == s.slot_size
    });
    if b.is_none() {
        b = new_block(g, st, s.type_bits, s.slot_size);
    }
    let Some(b) = b else {
        st.n_failed += 1;
        if st.n_failed <= 8 || st.n_failed.is_power_of_two() {
            crate::log::red(&format!(
                "sparse textures: no memory for a tile ({} failed, {} live)",
                st.n_failed, st.n_slots
            ));
        }
        return None;
    };
    let blk = &mut st.blocks[b];
    let idx = blk.busy.iter().position(|&x| x != 1).unwrap();
    let reused = blk.busy[idx] == 2;
    blk.busy[idx] = 1;
    blk.used += 1;
    if reused && blk.buf != vk::Buffer::null() {
        // a freed slot keeps its texels: zeroed again with the next fills
        FILLS
            .lock()
            .unwrap()
            .push((blk.buf, idx as u64 * blk.slot_size, blk.slot_size));
    }
    st.slots.insert(
        k,
        Slot {
            blk: b,
            idx,
            refs: 0,
        },
    );
    st.n_slots += 1;
    Some(k)
}

static FILLS: Mutex<Vec<(vk::Buffer, u64, u64)>> = Mutex::new(Vec::new());

fn slot_unref(st: &mut State, k: SlotKey) {
    if let Some(sl) = st.slots.get_mut(&k) {
        sl.refs -= 1;
        if sl.refs <= 0 {
            st.dying.push(k);
        }
    }
}

fn bind_page(g: &Gpu, st: &mut State, s: &mut Img, p: u64) {
    let layer = (p / s.stride as u64) as u32;
    let rel = (p % s.stride as u64) as u32;
    if layer >= s.layers {
        return;
    }
    let mut l = 0usize;
    while l + 1 < s.nnvn as usize && rel >= s.tile0[l + 1] {
        l += 1;
    }
    if l as u32 >= s.nlev {
        return;
    }
    let t = rel - s.tile0[l];
    let (tx, ty) = (t % s.tiles_x[l], t / s.tiles_x[l]);
    let (lw, lh) = ((s.w >> l).max(1), (s.h >> l).max(1));
    let pa = crate::nvn::vpool::translate(s.addr + p * PAGE, 1).map(|a| a & !(PAGE - 1));
    for sy in 0..s.nth / s.gh {
        for sx in 0..s.ntw / s.gw {
            let (x, y) = (tx * s.ntw + sx * s.gw, ty * s.nth + sy * s.gh);
            if x >= lw || y >= lh {
                continue;
            }
            let i = (layer * s.nblk + s.blk0[l] + (y / s.gh) * s.bx[l] + x / s.gw) as usize;
            let want = pa.and_then(|pa| slot_get(g, st, s, pa, sy * (s.ntw / s.gw) + sx));
            if want == s.bound[i] {
                continue;
            }
            if let Some(k) = want {
                if let Some(sl) = st.slots.get_mut(&k) {
                    sl.refs += 1;
                }
            }
            if let Some(old) = s.bound[i] {
                slot_unref(st, old);
            }
            s.bound[i] = want;
            if !s.dirty[i] {
                s.dirty[i] = true;
                s.dl.push(i as u32);
            }
        }
    }
}

fn bind_range(g: &Gpu, st: &mut State, s: &mut Img, lo: u64, hi: u64) {
    let (lo, hi) = (lo.max(s.addr), hi.min(s.addr + s.size));
    let mut a = lo & !(PAGE - 1);
    while a < hi {
        bind_page(g, st, s, (a - s.addr) / PAGE);
        a += PAGE;
    }
}

/// nvnMemoryPoolMapVirtual changed [lo, lo + size) of a sparse pool's range.
pub fn remap(lo: u64, size: u64) {
    let Some(g) = Gpu::try_get() else { return };
    let mut lk = ST.lock().unwrap();
    let Some(st) = lk.as_mut() else { return };
    let mut imgs = std::mem::take(&mut st.imgs);
    for s in imgs.iter_mut() {
        if lo < s.addr + s.size && s.addr < lo + size {
            bind_range(g, st, s, lo, lo + size);
        }
    }
    st.imgs = imgs;
}

pub fn is_sparse(img: vk::Image) -> bool {
    ST.lock()
        .unwrap()
        .as_ref()
        .is_some_and(|st| st.imgs.iter().any(|s| s.image == img))
}

/// The image goes (its slots are freed once the GPU is past it).
pub fn destroy(img: vk::Image) {
    let mut lk = ST.lock().unwrap();
    let Some(st) = lk.as_mut() else { return };
    let Some(i) = st.imgs.iter().position(|s| s.image == img) else {
        return;
    };
    let s = st.imgs.swap_remove(i);
    for k in s.bound.iter().flatten() {
        slot_unref(st, *k);
    }
    if let Some((k, _, _)) = s.tail {
        let last = st.tails.get_mut(&k).is_some_and(|(_, refs)| {
            *refs -= 1;
            *refs == 0
        });
        if last {
            if let Some((a, _)) = st.tails.remove(&k) {
                st.dying_tails.push(a);
            }
        }
    }
}

fn make_bind(s: &Img, i: u32, run: u32, st: &State) -> vk::SparseImageMemoryBind {
    let r = i % s.nblk;
    let layer = i / s.nblk;
    let mut l = 0usize;
    while l + 1 < s.nlev as usize && r >= s.blk0[l + 1] {
        l += 1;
    }
    let rr = r - s.blk0[l];
    let (bx, by) = (rr % s.bx[l], rr / s.bx[l]);
    let (x, y, w) = (bx * s.gw, by * s.gh, run * s.gw);
    let (lw, lh) = ((s.w >> l).max(1), (s.h >> l).max(1));
    let (mem, off) = match s.bound[i as usize].and_then(|k| st.slots.get(&k)) {
        Some(sl) => {
            let b = &st.blocks[sl.blk];
            (b.mem, b.base + sl.idx as u64 * b.slot_size)
        }
        None => (vk::DeviceMemory::null(), 0),
    };
    vk::SparseImageMemoryBind {
        subresource: vk::ImageSubresource {
            aspect_mask: s.aspect,
            mip_level: l as u32,
            array_layer: layer,
        },
        offset: vk::Offset3D {
            x: x as i32,
            y: y as i32,
            z: 0,
        },
        extent: vk::Extent3D {
            width: (lw - x).min(w),
            height: (lh - y).min(s.gh),
            depth: 1,
        },
        memory: mem,
        memory_offset: off,
        flags: vk::SparseMemoryBindFlags::empty(),
    }
}

/// Blocks i.. of one row bound to consecutive slots of one block (or all unbound): one bind.
fn run_len(s: &Img, st: &State, k: usize) -> u32 {
    let i = s.dl[k];
    let r = i % s.nblk;
    let mut l = 0usize;
    while l + 1 < s.nlev as usize && r >= s.blk0[l + 1] {
        l += 1;
    }
    let bx = (r - s.blk0[l]) % s.bx[l];
    let first = s.bound[i as usize]
        .and_then(|k| st.slots.get(&k))
        .map(|sl| (sl.blk, sl.idx));
    let mut run = 1u32;
    while k + (run as usize) < s.dl.len() && s.dl[k + run as usize] == i + run && bx + run < s.bx[l]
    {
        let nx = s.bound[(i + run) as usize]
            .and_then(|k| st.slots.get(&k))
            .map(|sl| (sl.blk, sl.idx));
        let ok = match (first, nx) {
            (Some((b, x)), Some((b2, x2))) => b == b2 && x2 == x + run as usize,
            (None, None) => true,
            _ => false,
        };
        if !ok {
            break;
        }
        run += 1;
    }
    run
}

/// Before a submission (queue lock held): the binds changed since the last
/// one (after every earlier submission: waits for `last`), and the frees of slots unbound before. Returns the sparse
/// timeline and the value the submission waits for (None: no sparse image yet).
/// The zero fills of new tile blocks and reused slots, into the submission's first command buffer (before it ends).
pub fn fills(g: &Gpu, cb: vk::CommandBuffer) {
    let mut lk = ST.lock().unwrap();
    let Some(st) = lk.as_mut() else { return };
    let mut fills = std::mem::take(&mut *FILLS.lock().unwrap());
    for b in st.blocks.iter_mut().filter(|b| b.fresh) {
        b.fresh = false;
        if b.buf != vk::Buffer::null() {
            fills.push((b.buf, 0, vk::WHOLE_SIZE));
        }
    }
    if !fills.is_empty() {
        for (buf, off, n) in fills {
            unsafe { g.dev().cmd_fill_buffer(cb, buf, off, n, 0) };
        }
        super::queue::full_barrier(g, cb);
    }
}

pub fn flush(
    g: &Gpu,
    last: u64,
    timeline_main: vk::Semaphore,
    fence_next: u64,
) -> Option<(vk::Semaphore, u64)> {
    let mut lk = ST.lock().unwrap();
    let bufs = std::mem::take(&mut *BUF_BINDS.lock().unwrap());
    if lk.is_none() && !bufs.is_empty() {
        *lk = Some(Default::default());
    }
    let st = lk.as_mut()?;
    let dev = g.dev();
    if st.timeline == vk::Semaphore::null() {
        let mut ti =
            vk::SemaphoreTypeCreateInfo::default().semaphore_type(vk::SemaphoreType::TIMELINE);
        let ci = vk::SemaphoreCreateInfo::default().push_next(&mut ti);
        st.timeline = unsafe { dev.create_semaphore(&ci, None) }.ok()?;
    }
    // slots unbound before: freed once this submission is done
    let dying = std::mem::take(&mut st.dying);
    if !dying.is_empty() {
        g.q.defer(
            fence_next,
            Box::new(move |_g: &Gpu| {
                let mut lk = ST.lock().unwrap();
                if let Some(st) = lk.as_mut() {
                    for k in dying {
                        if st.slots.get(&k).is_some_and(|s| s.refs <= 0) {
                            let sl = st.slots.remove(&k).unwrap();
                            let b = &mut st.blocks[sl.blk];
                            b.busy[sl.idx] = 2;
                            b.used -= 1;
                            st.n_slots -= 1;
                        }
                    }
                }
            }),
        );
    }
    let tails = std::mem::take(&mut st.dying_tails);
    if !tails.is_empty() {
        g.q.defer(
            fence_next,
            Box::new(move |g: &Gpu| {
                let mut a = g.alloc.lock().unwrap();
                for t in tails {
                    let _ = a.free(t);
                }
            }),
        );
    }
    let any = st.imgs.iter().any(|s| !s.dl.is_empty() || s.tail_pending) || !bufs.is_empty();
    if any {
        // pool buffers' granules, by buffer
        let mut by_buf: Vec<(vk::Buffer, Vec<vk::SparseMemoryBind>)> = Vec::new();
        for (b, m) in &bufs {
            match by_buf.iter_mut().find(|x| x.0 == *b) {
                Some(x) => x.1.push(*m),
                None => by_buf.push((*b, vec![*m])),
            }
        }
        let bb: Vec<vk::SparseBufferMemoryBindInfo> = by_buf
            .iter()
            .map(|(b, v)| {
                vk::SparseBufferMemoryBindInfo::default()
                    .buffer(*b)
                    .binds(v)
            })
            .collect();
        let mut binds: Vec<Vec<vk::SparseImageMemoryBind>> = Vec::new();
        let mut opaque: Vec<Vec<vk::SparseMemoryBind>> = Vec::new();
        let mut imgs_b: Vec<vk::Image> = Vec::new();
        let mut imgs_o: Vec<vk::Image> = Vec::new();
        let mut blocks = 0usize;
        let mut imgs = std::mem::take(&mut st.imgs);
        for s in imgs.iter_mut() {
            let mut v = Vec::new();
            if s.tail_pending {
                s.tail_pending = false;
                if let Some((mem, base)) = s.tail.as_ref().map(|(_, m, b)| (*m, *b)) {
                    let mut off = base;
                    for layer in 0..s.layers {
                        for l in s.nlev..s.tail_end {
                            let (lw, lh) = ((s.w >> l).max(1), (s.h >> l).max(1));
                            let mut y = 0;
                            while y < lh {
                                let mut x = 0;
                                while x < lw {
                                    v.push(vk::SparseImageMemoryBind {
                                        subresource: vk::ImageSubresource {
                                            aspect_mask: s.aspect,
                                            mip_level: l,
                                            array_layer: layer,
                                        },
                                        offset: vk::Offset3D {
                                            x: x as i32,
                                            y: y as i32,
                                            z: 0,
                                        },
                                        extent: vk::Extent3D {
                                            width: (lw - x).min(s.gw),
                                            height: (lh - y).min(s.gh),
                                            depth: 1,
                                        },
                                        memory: mem,
                                        memory_offset: off,
                                        flags: vk::SparseMemoryBindFlags::empty(),
                                    });
                                    off += s.slot_size;
                                    x += s.gw;
                                }
                                y += s.gh;
                            }
                        }
                    }
                    let nt = tail_count(s);
                    if nt > 0 {
                        let r = s.tail_req.unwrap();
                        let mut ob = Vec::new();
                        for t in 0..nt {
                            ob.push(vk::SparseMemoryBind {
                                resource_offset: r.image_mip_tail_offset
                                    + t as u64 * r.image_mip_tail_stride,
                                size: r.image_mip_tail_size,
                                memory: mem,
                                memory_offset: off,
                                flags: vk::SparseMemoryBindFlags::empty(),
                            });
                            off += round_up(r.image_mip_tail_size, s.slot_size);
                        }
                        opaque.push(ob);
                        imgs_o.push(s.image);
                    }
                }
            }
            if !s.dl.is_empty() {
                s.dl.sort_unstable();
                let mut k = 0;
                while k < s.dl.len() {
                    let run = run_len(s, st, k);
                    v.push(make_bind(s, s.dl[k], run, st));
                    for j in 0..run as usize {
                        let i = s.dl[k + j] as usize;
                        s.dirty[i] = false;
                    }
                    k += run as usize;
                }
                blocks += s.dl.len();
                s.dl.clear();
            }
            if !v.is_empty() {
                binds.push(v);
                imgs_b.push(s.image);
            }
        }
        st.imgs = imgs;
        let ib: Vec<vk::SparseImageMemoryBindInfo> = binds
            .iter()
            .zip(&imgs_b)
            .map(|(v, &im)| vk::SparseImageMemoryBindInfo::default().image(im).binds(v))
            .collect();
        let ob: Vec<vk::SparseImageOpaqueMemoryBindInfo> = opaque
            .iter()
            .zip(&imgs_o)
            .map(|(v, &im)| {
                vk::SparseImageOpaqueMemoryBindInfo::default()
                    .image(im)
                    .binds(v)
            })
            .collect();
        let sig = st.value + 1;
        let waits = [timeline_main];
        let wvals = [last];
        let sigs = [st.timeline];
        let svals = [sig];
        let mut tl = vk::TimelineSemaphoreSubmitInfo::default()
            .wait_semaphore_values(&wvals)
            .signal_semaphore_values(&svals);
        let bi = vk::BindSparseInfo::default()
            .wait_semaphores(&waits)
            .signal_semaphores(&sigs)
            .buffer_binds(&bb)
            .image_binds(&ib)
            .image_opaque_binds(&ob)
            .push_next(&mut tl);
        let t0 = std::time::Instant::now();
        if let Err(e) = unsafe { dev.queue_bind_sparse(g.d.queue, &[bi], vk::Fence::null()) } {
            super::lost::check(g, e, "vkQueueBindSparse");
            crate::log::red(&format!("vkQueueBindSparse: {e}"));
        } else {
            st.value = sig;
        }
        st.n_calls += 1;
        st.n_binds += blocks as u64;
        if st.n_calls <= 4 || st.n_calls.is_power_of_two() {
            crate::vklog!(
                "sparse bind {}: {blocks} blocks in {} images as {} binds, {} pool granules ({} blocks so far, {} tiles live = {} MB), {:.1} ms in the call",
                st.n_calls,
                ib.len(),
                binds.iter().map(|v| v.len()).sum::<usize>(),
                bufs.len(),
                st.n_binds,
                st.n_slots,
                (st.n_slots * st.blocks.first().map(|b| b.slot_size).unwrap_or(65536)) >> 20,
                t0.elapsed().as_secs_f64() * 1000.0
            );
        }
    }
    (st.value > 0).then_some((st.timeline, st.value))
}
