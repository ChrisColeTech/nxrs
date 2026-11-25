//! The pool descriptor set: NVN's texture and sampler pools as bindless arrays the shaders index by host ID (the
//! active pool's base plus the ID in the handle). Registration writes the descriptor at once (update-after-bind):
//! like the console's descriptor memory, an ID the GPU still reads must not be rewritten by the game.

use super::device::Device;
use super::layout::{self, Layout};
use crate::nvn::obj::{samp_par, view_t};
use ash::vk;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub struct Tables {
    pub pool: vk::DescriptorPool,
    pub set: vk::DescriptorSet,
    /// descriptor buffers (`caps.descbuf`): the pool set is this buffer, written in place (host-visible, mapped)
    pub db: Option<DescBuf>,
    inner: Mutex<Inner>,
    /// Registrations and image backing changes invalidate every dependent replay cache.
    generation: AtomicU64,
    /// Only actual descriptor-content changes invalidate immutable pool snapshots.
    pool_generation: AtomicU64,
    pending_images: AtomicBool,
}

/// One immutable version. The table caches only the latest; recorded submissions retain older versions through
/// their fences. Reusing a version across submissions does not mutate descriptors already visible to the GPU.
struct PoolSnapshot {
    generation: u64,
    dev: ash::Device,
    pool: vk::DescriptorPool,
    set: vk::DescriptorSet,
    db: Option<DescBuf>,
}
impl Drop for PoolSnapshot {
    fn drop(&mut self) {
        unsafe {
            if let Some(db) = &self.db {
                super::lost::ring::note_free(3, db.addr, db.size, vk::Handle::as_raw(db.buf), 0);
                self.dev.unmap_memory(db.mem);
                self.dev.destroy_buffer(db.buf, None);
                self.dev.free_memory(db.mem, None);
            } else {
                self.dev.destroy_descriptor_pool(self.pool, None);
            }
        }
    }
}

/// A host-visible descriptor buffer with its mapping and device address.
pub struct DescBuf {
    pub buf: vk::Buffer,
    pub mem: vk::DeviceMemory,
    pub ptr: usize,
    pub addr: u64,
    pub size: u64,
}

/// A descriptor buffer of `size` bytes in host-visible memory (device-local where the device has such: resizable BAR).
pub fn desc_buffer(d: &Device, size: u64) -> Result<DescBuf, String> {
    let usage = vk::BufferUsageFlags::RESOURCE_DESCRIPTOR_BUFFER_EXT
        | vk::BufferUsageFlags::SAMPLER_DESCRIPTOR_BUFFER_EXT
        | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS;
    let pair = super::queue::submission_safety::create_bound_buffer(
        || unsafe {
            d.dev.create_buffer(
                &vk::BufferCreateInfo::default().size(size.max(256)).usage(usage), None,
            )
        }.map_err(|e| format!("descriptor buffer: {e}")),
        |buf| {
            let req = unsafe { d.dev.get_buffer_memory_requirements(*buf) };
            let host = vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT;
            let ty = d.mem_type(req.memory_type_bits, host | vk::MemoryPropertyFlags::DEVICE_LOCAL)
                .or_else(|| d.mem_type(req.memory_type_bits, host))
                .ok_or("descriptor buffer: no host-visible memory")?;
            let mut flags = vk::MemoryAllocateFlagsInfo::default()
                .flags(vk::MemoryAllocateFlags::DEVICE_ADDRESS);
            let mem = unsafe {
                d.dev.allocate_memory(
                    &vk::MemoryAllocateInfo::default().allocation_size(req.size)
                        .memory_type_index(ty).push_next(&mut flags), None,
                )
            }.map_err(|e| format!("descriptor buffer memory: {e}"))?;
            Ok((mem, req.size))
        },
        |buf, (mem, _)| unsafe { d.dev.bind_buffer_memory(*buf, *mem, 0) }
            .map_err(|e| format!("descriptor buffer bind: {e}")),
        |buf| unsafe { d.dev.destroy_buffer(buf, None) },
        |(mem, _)| unsafe { d.dev.free_memory(mem, None) },
    )?;
    let (buf, (mem, allocation_size), ptr) = super::queue::submission_safety::map_bound_buffer(
        pair,
        |(mem, _)| unsafe { d.dev.map_memory(*mem, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty()) }
            .map(|p| p as usize).map_err(|e| format!("descriptor buffer map: {e}")),
        |buf| unsafe { d.dev.destroy_buffer(buf, None) },
        |(mem, _)| unsafe { d.dev.free_memory(mem, None) },
    )?;
    let addr = unsafe {
        d.dev
            .get_buffer_device_address(&vk::BufferDeviceAddressInfo::default().buffer(buf))
    };
    super::lost::ring::note_mem(13, addr, allocation_size, vk::Handle::as_raw(buf), 0, "");
    Ok(DescBuf {
        buf,
        mem,
        ptr,
        addr,
        size: allocation_size,
    })
}

/// Writes one descriptor (vkGetDescriptorEXT) at `at` bytes into a mapped descriptor buffer.
/// # Safety
/// `ptr + at` .. `+ size` lies inside the buffer's mapping.
pub unsafe fn put_descriptor(
    d: &Device,
    ptr: usize,
    at: u64,
    gi: &vk::DescriptorGetInfoEXT,
    size: usize,
) {
    let dst = core::slice::from_raw_parts_mut((ptr + at as usize) as *mut u8, size);
    d.db.get_descriptor(gi, dst);
}

#[derive(Default)]
struct Inner {
    /// Normal CPU memory: reading the live device-local mapping through BAR for a snapshot costs hundreds of ms.
    shadow: Vec<u8>,
    snapshot: Option<Arc<PoolSnapshot>>,
    image_desc: HashMap<i32, (vk::ImageView, Option<vk::ImageView>)>,
    prepare_images: HashSet<u64>,
    samplers: HashMap<[u32; 10], vk::Sampler>,
    tex_end: i32,
    /// Host base -> active NVN pool size; a pool-indexed shader can reach only its bound pool's IDs.
    tex_pools: HashMap<i32, i32>,
    samp_end: i32,
    /// host texture ID -> (serial, view) as registered
    tex: HashMap<i32, (u64, Option<view_t>)>,
    tex_versions: HashMap<i32, u64>,
    registration_epoch: u64,
    /// IDs whose finalized resource was replaced with a target-correct zero descriptor.
    placeholders: HashSet<i32>,
    /// IDs with a real storage-image descriptor; pool-indexed shader writes may reach any of these.
    storage_tex: BTreeMap<i32, vk::ImageView>,
    samp: HashMap<i32, vk::Sampler>,
    /// each sampler ID's filters as registered (min, mag; pass dumps)
    samp_filt: HashMap<i32, (u8, u8)>,
    /// buffer textures by host texture ID (no image: a texel buffer view over their pool's memory)
    buf_tex: HashMap<i32, crate::nvn::obj::tex_t>,
    /// buffer textures whose pool-set texel buffer views are still to be made (at the next submission)
    buf_pending: Vec<i32>,
    /// the pool set's texel buffer views by ID (destroyed when the ID is registered again)
    buf_views: HashMap<i32, (vk::BufferView, Option<vk::BufferView>)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TextureRegistration {
    pub id: i32,
    pub serial: u64,
    pub view: Option<view_t>,
    pub version: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CapturedTexture {
    pub registration: TextureRegistration,
    pub creation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CaptureEpoch {
    registrations: u64,
    images: u64,
}

impl Inner {
    fn registration(&self, id: i32) -> Option<TextureRegistration> {
        if self.placeholders.contains(&id) { return None; }
        let &(serial, view) = self.tex.get(&id)?;
        Some(TextureRegistration { id, serial, view, version: *self.tex_versions.get(&id)? })
    }

    fn bump_registration(&mut self, id: i32) {
        self.registration_epoch = self.registration_epoch.checked_add(1)
            .unwrap_or_else(|| super::queue::submission_safety::resource_safety_fatal());
        self.tex_versions.insert(id, self.registration_epoch);
    }
}

fn samp_key(p: &samp_par) -> [u32; 10] {
    [
        u32::from_le_bytes([p.minf, p.magf, p.ws, p.wt]),
        u32::from_le_bytes([p.wr, p.cmp_mode, p.cmp_func, p.reduction]),
        p.lod_min.to_bits(),
        p.lod_max.to_bits(),
        p.lod_bias.to_bits(),
        p.aniso.to_bits(),
        p.border[0].to_bits(),
        p.border[1].to_bits(),
        p.border[2].to_bits(),
        p.border[3].to_bits(),
    ]
}

#[cfg(test)]
mod registration_boundary_tests {
    use super::*;
    use std::cell::Cell;

    fn cpu_tables() -> Tables {
        Tables { pool: vk::DescriptorPool::null(), set: vk::DescriptorSet::null(), db: None,
            inner: Mutex::new(Inner::default()), generation: AtomicU64::new(1),
            pool_generation: AtomicU64::new(1), pending_images: AtomicBool::new(false) }
    }

    #[test]
    fn capture_rejects_reregistration_even_with_same_id_serial_and_creation() {
        let tables = cpu_tables();
        let canonical = view_t::default();
        for changed in [
            view_t { bits: 1, base_level: 1, nlevels: 1, ..canonical },
            view_t { bits: 2, min_layer: 1, nlayers: 1, ..canonical },
            view_t { bits: 16, swz: [2, 1, 0, 3], ..canonical },
            canonical, // same interpretation, different registration lifetime
        ] {
            tables.note_tex(9, 42, Some(changed));
            let captured = tables.capture_texture(9, |_| {
                assert!(tables.inner.try_lock().is_ok(), "capture held tables while observing images");
                Some(77)
            }).unwrap();
            tables.note_tex(9, 42, Some(canonical));
            let called = Cell::new(false);
            assert!(tables.prepare_captured(&[captured], || { called.set(true); Ok(()) }).is_err());
            assert!(!called.get(), "changed capture must refuse before preparation");
        }
        tables.note_tex(9, 42, None);
        assert!(tables.capture_texture(9, |_| {
            tables.note_tex(9, 42, None); // mutation in the image-observation window
            Some(77)
        }).is_none());
    }

    #[test]
    fn preparation_failure_or_epoch_change_cannot_publish_a_stable_shadow() {
        struct Owner<'a>(&'a Cell<u32>);
        impl Drop for Owner<'_> { fn drop(&mut self) { self.0.set(self.0.get() + 1); } }
        let tables = cpu_tables();
        tables.note_tex(9, 42, None);
        let capture = tables.capture_texture(9, |_| Some(77)).unwrap();
        let dropped = Cell::new(0);
        let changed = tables.prepare_captured(&[capture], || {
            assert!(tables.inner.try_lock().is_ok(), "preparation may acquire images without inversion");
            tables.note_tex(9, 42, Some(view_t { bits: 1, base_level: 1, ..Default::default() }));
            tables.note_tex(9, 42, None); // ABA must not restore permission
            Ok(Owner(&dropped))
        });
        assert!(changed.is_err()); assert_eq!(dropped.get(), 1);
        let capture = tables.capture_texture(9, |_| Some(77)).unwrap();
        let failed: Result<((), CaptureEpoch), _> = tables.prepare_captured(&[capture], || Err("injected allocation failure".into()));
        assert_eq!(failed.unwrap_err(), "injected allocation failure");
        let (_, epoch) = tables.prepare_captured(&[capture], || Ok(())).unwrap();
        assert!(tables.registration_unchanged(epoch)); // issue boundary
        tables.note_tex(9, 42, None); // mutation during guest replay
        assert!(!tables.registration_unchanged(epoch)); // comparison must be withheld
        let capture = tables.capture_texture(9, |_| Some(77)).unwrap();
        assert!(tables.prepare_captured(&[capture], || {
            tables.invalidate_images(); // backing replacement, without re-registering the slot
            Ok(())
        }).is_err());
    }
}

fn wrap(w: u8) -> vk::SamplerAddressMode {
    match w {
        0 | 7 => vk::SamplerAddressMode::CLAMP_TO_EDGE,
        1 => vk::SamplerAddressMode::REPEAT,
        2 | 3 | 4 => vk::SamplerAddressMode::MIRROR_CLAMP_TO_EDGE,
        5 => vk::SamplerAddressMode::CLAMP_TO_BORDER,
        6 => vk::SamplerAddressMode::MIRRORED_REPEAT,
        _ => vk::SamplerAddressMode::REPEAT,
    }
}

/// NVN compare functions (NEVER = 1 .. ALWAYS = 8) as Vulkan's (NEVER = 0 .. ALWAYS = 7).
pub fn compare_op(f: u32) -> vk::CompareOp {
    vk::CompareOp::from_raw((f.clamp(1, 8) - 1) as i32)
}

impl Tables {
    pub fn new(d: &Device, l: &Layout) -> Result<Tables, String> {
        if d.caps.descbuf {
            let db = desc_buffer(d, l.db_pool_size)?;
            unsafe { core::ptr::write_bytes(db.ptr as *mut u8, 0, l.db_pool_size as usize) };
            return Ok(Tables {
                pool: vk::DescriptorPool::null(),
                set: vk::DescriptorSet::null(),
                db: Some(db),
                inner: Mutex::new(Inner {
                    shadow: vec![0; l.db_pool_size as usize],
                    ..Inner::default()
                }),
                generation: AtomicU64::new(1),
                pool_generation: AtomicU64::new(1),
                pending_images: AtomicBool::new(false),
            });
        }
        let sizes = [
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::SAMPLED_IMAGE,
                descriptor_count: l.tex_cap,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::SAMPLER,
                descriptor_count: l.samp_cap,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::STORAGE_IMAGE,
                descriptor_count: l.tex_cap,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::UNIFORM_TEXEL_BUFFER,
                descriptor_count: l.tex_cap,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::STORAGE_TEXEL_BUFFER,
                descriptor_count: l.tex_cap,
            },
        ];
        let ci = vk::DescriptorPoolCreateInfo::default()
            .flags(vk::DescriptorPoolCreateFlags::UPDATE_AFTER_BIND)
            .max_sets(1)
            .pool_sizes(&sizes);
        let pool = unsafe { d.dev.create_descriptor_pool(&ci, None) }
            .map_err(|e| format!("pool set pool: {e}"))?;
        let sl = [l.pool_set];
        let ai = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(pool)
            .set_layouts(&sl);
        let set = unsafe { d.dev.allocate_descriptor_sets(&ai) }
            .map_err(|e| format!("pool set: {e}"))?[0];
        Ok(Tables {
            pool,
            set,
            db: None,
            inner: Mutex::new(Inner::default()),
            generation: AtomicU64::new(1),
            pool_generation: AtomicU64::new(1),
            pending_images: AtomicBool::new(false),
        })
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    pub fn invalidate_images(&self) {
        self.generation.fetch_add(1, Ordering::Release);
    }

    pub fn pool_generation(&self) -> u64 {
        self.pool_generation.load(Ordering::Acquire)
    }

    pub fn note_image_pending(&self, serial: u64) {
        let mut i = self.inner.lock().unwrap();
        i.prepare_images.insert(serial);
        self.pending_images.store(true, Ordering::Release);
    }

    pub fn has_pending_images(&self) -> bool {
        self.pending_images.load(Ordering::Acquire)
    }

    pub fn take_image_pending(&self) -> Vec<u64> {
        let mut i = self.inner.lock().unwrap();
        self.pending_images.store(false, Ordering::Release);
        i.prepare_images.drain().collect()
    }

    /// Called under inner after publishing descriptor bytes/sets, never for image ownership or materialization
    /// alone. Stage caches still use the broader registration/backing generation.
    fn descriptors_changed(&self) {
        self.invalidate_images();
        self.pool_generation.fetch_add(1, Ordering::Release);
    }

    unsafe fn put_pool_descriptor(
        &self,
        d: &Device,
        i: &mut Inner,
        at: u64,
        gi: &vk::DescriptorGetInfoEXT,
        size: usize,
    ) {
        let bytes = &mut i.shadow[at as usize..at as usize + size];
        d.db.get_descriptor(gi, bytes);
        core::ptr::copy_nonoverlapping(
            bytes.as_ptr(),
            (self.db.as_ref().unwrap().ptr + at as usize) as *mut u8,
            size,
        );
    }

    /// Freeze the pool for recorded commands. Updating the live pool after an inline image upgrade must not
    /// redirect earlier draws to the replacement image before the ordered copy reaches it.
    pub fn snapshot(&self, g: &super::Gpu) -> (u64, vk::DescriptorSet, u64) {
        let start = std::time::Instant::now();
        let mut i = self.inner.lock().unwrap();
        let generation = self.pool_generation();
        let snapshot = if let Some(s) = i.snapshot.as_ref().filter(|s| s.generation == generation) {
            super::perf::inc(super::perf::C::PoolSnapshotReused);
            s.clone()
        } else {
            super::perf::inc(super::perf::C::PoolSnapshots);
            let (pool, set, db) = if self.db.is_some() {
                let dst = desc_buffer(&g.d, g.layout.db_pool_size).expect("texture pool snapshot");
                super::perf::add(super::perf::C::PoolSnapshotBytes, g.layout.db_pool_size);
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        i.shadow.as_ptr(),
                        dst.ptr as *mut u8,
                        i.shadow.len(),
                    )
                };
                (
                    vk::DescriptorPool::null(),
                    vk::DescriptorSet::null(),
                    Some(dst),
                )
            } else {
                let copy = Self::new(&g.d, &g.layout).expect("texture pool snapshot");
                // Copying an undefined descriptor leaves the destination undefined too (unused pool slots).
                let copies: Vec<_> = [
                    (layout::B_TEX, g.layout.tex_cap),
                    (layout::B_SAMP, g.layout.samp_cap),
                    (layout::B_IMG, g.layout.tex_cap),
                    (layout::B_TBUF, g.layout.tex_cap),
                    (layout::B_IBUF, g.layout.tex_cap),
                ]
                .into_iter()
                .map(|(binding, count)| {
                    vk::CopyDescriptorSet::default()
                        .src_set(self.set)
                        .src_binding(binding)
                        .dst_set(copy.set)
                        .dst_binding(binding)
                        .descriptor_count(count)
                })
                .collect();
                unsafe { g.dev().update_descriptor_sets(&[], &copies) };
                (copy.pool, copy.set, None)
            };
            let s = Arc::new(PoolSnapshot {
                generation,
                dev: g.dev().clone(),
                pool,
                set,
                db,
            });
            i.snapshot = Some(s.clone());
            s
        };
        let (set, addr) = (snapshot.set, snapshot.db.as_ref().map_or(0, |db| db.addr));
        drop(i);
        g.q.defer_after_use(Box::new(move |_| drop(snapshot)));
        super::perf::add(
            super::perf::C::PoolSnapshotNs,
            start.elapsed().as_nanos() as u64,
        );
        (generation, set, addr)
    }

    /// A buffer texture registered at host ID `id`.
    pub fn note_buf_tex(&self, id: i32, t: crate::nvn::obj::tex_t) {
        let mut i = self.inner.lock().unwrap();
        i.bump_registration(id);
        i.tex.remove(&id);
        i.tex_versions.remove(&id);
        i.storage_tex.remove(&id);
        i.image_desc.remove(&id);
        i.buf_tex.insert(id, t);
        i.buf_pending.push(id);
        self.invalidate_images();
    }
    /// The buffer textures over [lo, hi) are written again at the next submission: their memory pool went away or a
    /// new one took its place (their descriptors name the old buffer's device address: a write through one faulted).
    pub fn requeue_buf_texs(&self, lo: u64, hi: u64) {
        let mut i = self.inner.lock().unwrap();
        let ids: Vec<i32> = i
            .buf_tex
            .iter()
            .filter(|(_, t)| (t.mem as u64) < hi && t.mem as u64 + (t.d.width as u64).max(1) > lo)
            .map(|(k, _)| *k)
            .collect();
        i.buf_pending.extend(ids);
    }
    pub fn buf_tex_at(&self, id: i32) -> Option<crate::nvn::obj::tex_t> {
        self.inner.lock().unwrap().buf_tex.get(&id).copied()
    }
    pub fn take_buf_pending(&self) -> Vec<i32> {
        std::mem::take(&mut self.inner.lock().unwrap().buf_pending)
    }
    /// Writes the pool set's texel buffer views of buffer texture `id` (uniform, and storage when the format allows).
    /// Descriptor buffers: buffer texture `id`'s texel buffer descriptors (uniform, and storage when the format allows)
    /// from its memory's device address.
    pub fn write_texel_addrs(
        &self,
        d: &Device,
        l: &Layout,
        id: i32,
        u: vk::DescriptorAddressInfoEXT,
        s: Option<vk::DescriptorAddressInfoEXT>,
    ) {
        if self.db.is_none() {
            return;
        }
        let p = &d.db_props;
        let mut i = self.inner.lock().unwrap();
        unsafe {
            let gi = vk::DescriptorGetInfoEXT::default()
                .ty(vk::DescriptorType::UNIFORM_TEXEL_BUFFER)
                .data(vk::DescriptorDataEXT {
                    p_uniform_texel_buffer: &u,
                });
            self.put_pool_descriptor(
                d,
                &mut i,
                l.db_pool_off[layout::B_TBUF as usize]
                    + id as u64
                        * super::device::desc_size(p, vk::DescriptorType::UNIFORM_TEXEL_BUFFER)
                            as u64,
                &gi,
                super::device::desc_size(p, vk::DescriptorType::UNIFORM_TEXEL_BUFFER),
            );
            if let Some(s) = s {
                let gi = vk::DescriptorGetInfoEXT::default()
                    .ty(vk::DescriptorType::STORAGE_TEXEL_BUFFER)
                    .data(vk::DescriptorDataEXT {
                        p_storage_texel_buffer: &s,
                    });
                self.put_pool_descriptor(
                    d,
                    &mut i,
                    l.db_pool_off[layout::B_IBUF as usize]
                        + id as u64
                            * super::device::desc_size(p, vk::DescriptorType::STORAGE_TEXEL_BUFFER)
                                as u64,
                    &gi,
                    super::device::desc_size(p, vk::DescriptorType::STORAGE_TEXEL_BUFFER),
                );
            }
        }
        self.descriptors_changed();
    }

    pub fn write_texel_views(
        &self,
        d: &Device,
        id: i32,
        uv: vk::BufferView,
        sv: Option<vk::BufferView>,
    ) {
        let u = [uv];
        let s = [sv.unwrap_or(uv)];
        let mut w = vec![vk::WriteDescriptorSet::default()
            .dst_set(self.set)
            .dst_binding(layout::B_TBUF)
            .dst_array_element(id as u32)
            .descriptor_type(vk::DescriptorType::UNIFORM_TEXEL_BUFFER)
            .texel_buffer_view(&u)];
        if sv.is_some() {
            w.push(
                vk::WriteDescriptorSet::default()
                    .dst_set(self.set)
                    .dst_binding(layout::B_IBUF)
                    .dst_array_element(id as u32)
                    .descriptor_type(vk::DescriptorType::STORAGE_TEXEL_BUFFER)
                    .texel_buffer_view(&s),
            );
        }
        let mut g = self.inner.lock().unwrap();
        unsafe { d.dev.update_descriptor_sets(&w, &[]) };
        self.descriptors_changed();
        if let Some((ou, os)) = g.buf_views.insert(id, (uv, sv)) {
            drop(g);
            if let Some(gpu) = super::Gpu::try_get() {
                gpu.q
                    .defer_after_use(Box::new(move |gpu: &super::Gpu| unsafe {
                        gpu.dev().destroy_buffer_view(ou, None);
                        if let Some(os) = os {
                            gpu.dev().destroy_buffer_view(os, None);
                        }
                    }));
            }
        }
    }

    pub fn reserve_tex(&self, base: i32, n: i32) {
        let g = super::Gpu::try_get();
        let cap = g.map(|g| g.layout.tex_cap as i32).unwrap_or(i32::MAX);
        let mut i = self.inner.lock().unwrap();
        i.tex_end = i.tex_end.max(base + n);
        i.tex_pools.insert(base, n);
        if base + n > cap {
            crate::log::red(&format!(
                "texture pool IDs up to {} pass the pool set's {cap} (NXGFX_TEX_CAP)",
                base + n
            ));
        }
    }
    pub fn reserve_samp(&self, base: i32, n: i32) {
        let g = super::Gpu::try_get();
        let cap = g.map(|g| g.layout.samp_cap as i32).unwrap_or(i32::MAX);
        let mut i = self.inner.lock().unwrap();
        i.samp_end = i.samp_end.max(base + n);
        if base + n > cap {
            crate::log::red(&format!(
                "sampler pool IDs up to {} pass the pool set's {cap} (NXGFX_SAMP_CAP)",
                base + n
            ));
        }
    }

    /// The (shared) Vulkan sampler of NVN sampler parameters.
    pub fn sampler(&self, d: &Device, p: &samp_par) -> vk::Sampler {
        let k = samp_key(p);
        let mut i = self.inner.lock().unwrap();
        if let Some(s) = i.samplers.get(&k) {
            return *s;
        }
        let (min, mip, nomip) = match p.minf {
            0 => (vk::Filter::NEAREST, vk::SamplerMipmapMode::NEAREST, true),
            1 => (vk::Filter::LINEAR, vk::SamplerMipmapMode::NEAREST, true),
            2 => (vk::Filter::NEAREST, vk::SamplerMipmapMode::NEAREST, false),
            3 => (vk::Filter::LINEAR, vk::SamplerMipmapMode::NEAREST, false),
            4 => (vk::Filter::NEAREST, vk::SamplerMipmapMode::LINEAR, false),
            _ => (vk::Filter::LINEAR, vk::SamplerMipmapMode::LINEAR, false),
        };
        let mag = if p.magf == 0 {
            vk::Filter::NEAREST
        } else {
            vk::Filter::LINEAR
        };
        // NXGFX_SAMP_LINEAR=1 (diagnosis): every sampler filters linearly (is a picture's banding a nearest read?)
        static LIN: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let lin = *LIN.get_or_init(|| std::env::var("NXGFX_SAMP_LINEAR").as_deref() == Ok("1"));
        let (min, mag) = if lin {
            (vk::Filter::LINEAR, vk::Filter::LINEAR)
        } else {
            (min, mag)
        };
        let min_lod = p.lod_min.max(0.0);
        let max_lod = if nomip {
            min_lod + 0.25
        } else {
            p.lod_max.max(min_lod)
        };
        let aniso = p.aniso > 1.0;
        let border_rgba = p.border;
        let mut cbc = vk::SamplerCustomBorderColorCreateInfoEXT::default()
            .custom_border_color(vk::ClearColorValue {
                float32: border_rgba,
            })
            .format(vk::Format::UNDEFINED);
        let standard = |b: [f32; 4]| {
            if b[3] == 0.0 {
                vk::BorderColor::FLOAT_TRANSPARENT_BLACK
            } else if b[0] == 0.0 && b[1] == 0.0 && b[2] == 0.0 {
                vk::BorderColor::FLOAT_OPAQUE_BLACK
            } else {
                vk::BorderColor::FLOAT_OPAQUE_WHITE
            }
        };
        let is_std = border_rgba == [0.0; 4]
            || border_rgba == [0.0, 0.0, 0.0, 1.0]
            || border_rgba == [1.0; 4];
        let mut ci = vk::SamplerCreateInfo::default()
            .mag_filter(mag)
            .min_filter(min)
            .mipmap_mode(mip)
            .address_mode_u(wrap(p.ws))
            .address_mode_v(wrap(p.wt))
            .address_mode_w(wrap(p.wr))
            .mip_lod_bias(p.lod_bias.clamp(-15.0, 15.0))
            .anisotropy_enable(aniso)
            .max_anisotropy(p.aniso.clamp(1.0, 16.0))
            .compare_enable(p.cmp_mode != 0)
            .compare_op(compare_op(p.cmp_func as u32))
            .min_lod(min_lod)
            .max_lod(max_lod)
            .border_color(if is_std || !d.caps.custom_border {
                standard(border_rgba)
            } else {
                vk::BorderColor::FLOAT_CUSTOM_EXT
            });
        if !is_std && d.caps.custom_border {
            ci = ci.push_next(&mut cbc);
        }
        let mut red =
            vk::SamplerReductionModeCreateInfo::default().reduction_mode(match p.reduction {
                1 => vk::SamplerReductionMode::MIN,
                2 => vk::SamplerReductionMode::MAX,
                _ => vk::SamplerReductionMode::WEIGHTED_AVERAGE,
            });
        if p.reduction != 0 {
            ci = ci.push_next(&mut red);
        }
        let s = unsafe { d.dev.create_sampler(&ci, None) }
            .unwrap_or_else(|e| panic!("vkCreateSampler: {e}"));
        if i.samplers.len() == 3500 {
            crate::log::red(
                "3,500 distinct samplers: close to the driver's limit of sampler objects",
            );
        }
        i.samplers.insert(k, s);
        s
    }

    pub fn note_tex(&self, id: i32, serial: u64, view: Option<view_t>) {
        let mut i = self.inner.lock().unwrap();
        // Re-registering the same interpretation is still a new capture lifetime.
        i.bump_registration(id);
        i.placeholders.remove(&id);
        if i.tex.get(&id) == Some(&(serial, view)) {
            return;
        }
        // A different object's view can reuse a retired Vulkan handle. Equality of handles is a sufficient
        // no-op check only within the same registration lifetime.
        i.image_desc.remove(&id);
        i.tex.insert(id, (serial, view));
        i.storage_tex.remove(&id);
        self.invalidate_images();
    }
    /// The texture IDs registered with one of `serials`: (ID, serial, view).
    pub fn ids_of(
        &self,
        serials: &std::collections::HashSet<u64>,
    ) -> Vec<(i32, u64, Option<view_t>)> {
        if serials.is_empty() {
            return Vec::new();
        }
        self.inner
            .lock()
            .unwrap()
            .tex
            .iter()
            .filter(|(_, (s, _))| serials.contains(s))
            .map(|(id, (s, v))| (*id, *s, *v))
            .collect()
    }
    pub fn tex_at(&self, id: i32) -> Option<(u64, Option<view_t>)> {
        self.inner.lock().unwrap().tex.get(&id).copied()
    }

    pub(crate) fn registration_at(&self, id: i32) -> Option<TextureRegistration> {
        self.inner.lock().unwrap().registration(id)
    }

    /// Never hold the table mutex while observing images (image code takes tables
    /// under the images mutex). A change during the callback invalidates capture.
    pub(crate) fn capture_texture(&self, id: i32, creation: impl FnOnce(u64) -> Option<u64>)
        -> Option<CapturedTexture> {
        let registration = self.registration_at(id)?;
        let creation = creation(registration.serial)?;
        if creation == 0 || self.registration_at(id) != Some(registration) { return None; }
        Some(CapturedTexture { registration, creation })
    }

    /// Optimistic preparation transaction. Its callback cannot record commands;
    /// caller rechecks the epoch before issue and after guest replay before comparing.
    pub(crate) fn prepare_captured<T>(&self, captures: &[CapturedTexture],
        prepare: impl FnOnce() -> Result<T, String>) -> Result<(T, CaptureEpoch), String> {
        let epoch = {
            let inner = self.inner.lock().unwrap();
            if captures.iter().any(|c| inner.registration(c.registration.id) != Some(c.registration)) {
                return Err("NDVE captured registration changed".into());
            }
            CaptureEpoch { registrations: inner.registration_epoch, images: self.generation() }
        };
        let prepared = prepare()?;
        if !self.registration_unchanged(epoch) {
            return Err("NDVE registration/backing changed during preparation".into());
        }
        Ok((prepared, epoch))
    }

    pub(crate) fn registration_unchanged(&self, epoch: CaptureEpoch) -> bool {
        self.inner.lock().unwrap().registration_epoch == epoch.registrations
            && self.generation() == epoch.images
    }
    pub fn placeholder_at(&self, id: i32) -> bool {
        self.inner.lock().unwrap().placeholders.contains(&id)
    }
    pub fn mark_placeholder(&self, id: i32) {
        let mut i = self.inner.lock().unwrap();
        i.bump_registration(id);
        i.placeholders.insert(id);
        self.invalidate_images();
    }
    pub fn texture_slots(&self) -> Vec<(i32, u64, Option<view_t>)> {
        self.inner
            .lock()
            .unwrap()
            .tex
            .iter()
            .map(|(&id, &(s, v))| (id, s, v))
            .collect()
    }
    pub fn storage_views(&self, base: i32) -> Vec<(u64, vk::ImageView)> {
        let i = self.inner.lock().unwrap();
        let end = base + i.tex_pools.get(&base).copied().unwrap_or(0);
        i.storage_tex
            .range(base..end)
            .filter_map(|(id, &v)| i.tex.get(id).map(|&(s, _)| (s, v)))
            .collect()
    }
    /// A sampler ID's (min, mag) filters as registered (NVN values).
    pub fn samp_filters(&self, id: i32) -> Option<(u8, u8)> {
        self.inner.lock().unwrap().samp_filt.get(&id).copied()
    }
    pub fn samp_at(&self, id: i32) -> Option<vk::Sampler> {
        self.inner.lock().unwrap().samp.get(&id).copied()
    }

    pub fn write_sampler(&self, d: &Device, id: i32, s: vk::Sampler) {
        let mut i = self.inner.lock().unwrap();
        if i.samp.get(&id) == Some(&s) {
            return;
        }
        i.samp.insert(id, s);
        if self.db.is_some() {
            let Some(g) = super::Gpu::try_get() else {
                return;
            };
            let p = &d.db_props;
            let gi = vk::DescriptorGetInfoEXT::default()
                .ty(vk::DescriptorType::SAMPLER)
                .data(vk::DescriptorDataEXT { p_sampler: &s });
            unsafe {
                self.put_pool_descriptor(
                    d,
                    &mut i,
                    g.layout.db_pool_off[layout::B_SAMP as usize]
                        + id as u64 * p.sampler_descriptor_size as u64,
                    &gi,
                    p.sampler_descriptor_size,
                )
            };
            self.descriptors_changed();
            return;
        }
        let ii = [vk::DescriptorImageInfo::default().sampler(s)];
        let w = [vk::WriteDescriptorSet::default()
            .dst_set(self.set)
            .dst_binding(layout::B_SAMP)
            .dst_array_element(id as u32)
            .descriptor_type(vk::DescriptorType::SAMPLER)
            .image_info(&ii)];
        unsafe { d.dev.update_descriptor_sets(&w, &[]) };
        self.descriptors_changed();
    }

    pub fn write_image(
        &self,
        d: &Device,
        id: i32,
        view: vk::ImageView,
        storage: Option<vk::ImageView>,
    ) {
        let mut i = self.inner.lock().unwrap();
        if let Some(v) = storage {
            i.storage_tex.insert(id, v);
        } else {
            i.storage_tex.remove(&id);
        }
        if i.image_desc.get(&id) == Some(&(view, storage)) {
            return;
        }
        i.image_desc.insert(id, (view, storage));
        if self.db.is_some() {
            let Some(g) = super::Gpu::try_get() else {
                return;
            };
            let p = &d.db_props;
            let ii = vk::DescriptorImageInfo::default()
                .image_view(view)
                .image_layout(vk::ImageLayout::GENERAL);
            unsafe {
                let gi = vk::DescriptorGetInfoEXT::default()
                    .ty(vk::DescriptorType::SAMPLED_IMAGE)
                    .data(vk::DescriptorDataEXT {
                        p_sampled_image: &ii,
                    });
                self.put_pool_descriptor(
                    d,
                    &mut i,
                    g.layout.db_pool_off[layout::B_TEX as usize]
                        + id as u64 * p.sampled_image_descriptor_size as u64,
                    &gi,
                    p.sampled_image_descriptor_size,
                );
                let si = vk::DescriptorImageInfo::default()
                    .image_view(storage.unwrap_or(view))
                    .image_layout(vk::ImageLayout::GENERAL);
                if storage.is_some() {
                    let gi = vk::DescriptorGetInfoEXT::default()
                        .ty(vk::DescriptorType::STORAGE_IMAGE)
                        .data(vk::DescriptorDataEXT {
                            p_storage_image: &si,
                        });
                    self.put_pool_descriptor(
                        d,
                        &mut i,
                        g.layout.db_pool_off[layout::B_IMG as usize]
                            + id as u64 * p.storage_image_descriptor_size as u64,
                        &gi,
                        p.storage_image_descriptor_size,
                    );
                }
            }
            self.descriptors_changed();
            return;
        }
        let ii = [vk::DescriptorImageInfo::default()
            .image_view(view)
            .image_layout(vk::ImageLayout::GENERAL)];
        let si = [vk::DescriptorImageInfo::default()
            .image_view(storage.unwrap_or(view))
            .image_layout(vk::ImageLayout::GENERAL)];
        let mut w = vec![vk::WriteDescriptorSet::default()
            .dst_set(self.set)
            .dst_binding(layout::B_TEX)
            .dst_array_element(id as u32)
            .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
            .image_info(&ii)];
        if storage.is_some() {
            w.push(
                vk::WriteDescriptorSet::default()
                    .dst_set(self.set)
                    .dst_binding(layout::B_IMG)
                    .dst_array_element(id as u32)
                    .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                    .image_info(&si),
            );
        }
        unsafe { d.dev.update_descriptor_sets(&w, &[]) };
        self.descriptors_changed();
    }
}

impl super::Gpu {
    /// nvnSamplerPoolRegisterSampler at host ID `id`.
    pub fn reg_samp(&self, id: i32, p: &samp_par) {
        super::capture::samp_reg(id, p);
        if id < 0 || id as u32 >= self.layout.samp_cap {
            crate::log::red_once(&format!("sampler ID {id} past the pool set"));
            return;
        }
        let s = self.tables.sampler(&self.d, p);
        self.tables
            .inner
            .lock()
            .unwrap()
            .samp_filt
            .insert(id, (p.minf, p.magf));
        self.tables.write_sampler(&self.d, id, s);
    }
}
