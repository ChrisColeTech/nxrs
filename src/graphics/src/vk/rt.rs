//! NVN2 ray tracing on Vulkan: acceleration structures built from NVN's own build descriptions
//! (`nvn::query::AsBuild`: triangles, boxes, instances) as VK_KHR_acceleration_structure objects, keyed by the guest
//! address the game builds at (`destination`); a top level's instance records name their bottom levels by that guest
//! address, patched to the host structure's device address. Lane S's ray-tracing library traces with ray queries
//! against them (`rt_scene`).
//!
//! What is not here yet: the NVN2 command entry points (`nvnCommandBufferBuildAccelerationStructure`, `…DispatchRays`,
//! copy / compact / relocate, binds): their argument layouts are not known (no symbolized implementation in the owner's
//! games; nxnative has none). They call `build` and `lookup` once known (PORT_PLAN 5A.4).

use super::queue::Sub;
use super::Gpu;
use crate::nvn::query::{AsBuild, AsGeometry};
use ash::vk;
use gpu_allocator::MemoryLocation;
use std::collections::HashMap;
use std::sync::Mutex;

/// A host acceleration structure (its buffer is kept with it).
pub struct Accel {
    pub handle: vk::AccelerationStructureKHR,
    pub buf: super::Buf,
    /// its device address (instance records and ray queries name it by this)
    pub addr: u64,
    pub top: bool,
}
unsafe impl Send for Accel {}

static ACCELS: Mutex<Option<HashMap<u64, Accel>>> = Mutex::new(None);

/// The host structure built at guest address `a`: (handle, device address).
pub fn lookup(a: u64) -> Option<(vk::AccelerationStructureKHR, u64)> {
    ACCELS
        .lock()
        .unwrap()
        .as_ref()?
        .get(&a)
        .map(|x| (x.handle, x.addr))
}

fn dev_addr(g: &Gpu, sub: &mut Sub, a: u64, n: u64) -> Option<u64> {
    let (b, off, _) = super::mem::resolve(g, sub, a, n.max(1))?;
    super::mem::buffer_address(b).map(|s| s + off)
}

/// NVN vertex format of a triangle geometry → Vulkan (position formats only).
fn vertex_format(f: u32) -> Option<vk::Format> {
    Some(match f {
        34 | 46 => vk::Format::R32G32B32_SFLOAT, // RGB32F / RGBA32F (x, y, z read)
        22 => vk::Format::R32G32_SFLOAT,
        _ => return None,
    })
}

/// Records the build of `b` into `sub` (its scratch kept with the submission), replacing a structure built at the same
/// destination before. Instances must name structures built earlier (in this or an earlier submission).
pub fn build(g: &Gpu, sub: &mut Sub, b: &AsBuild) -> Result<(), String> {
    if !g.d.caps.rt {
        return Err("the device has no acceleration structures".into());
    }
    if b.count != 0 && b.geometries.is_null() {
        return Err("no geometries".into());
    }
    let geos: Vec<AsGeometry> = (0..b.count as usize)
        .map(|i| unsafe { *b.geometries.add(i) })
        .collect();
    let top = geos.iter().any(|g| g.kind == 3);
    if top && geos.len() != 1 {
        return Err("a top level has one instance geometry".into());
    }
    let mut vk_geos = Vec::new();
    let mut prims = Vec::new();
    let mut keep: Vec<super::Buf> = Vec::new();
    for ge in &geos {
        match ge.kind {
            1 => {
                let (va, vcount, vfmt, stride) = (
                    ge.w[0],
                    (ge.w[1] & 0xffff_ffff) as u32,
                    (ge.w[1] >> 32) as u32,
                    ge.w[2],
                );
                let (ia, icount, ity) = (
                    ge.w[3],
                    (ge.w[4] & 0xffff_ffff) as u32,
                    (ge.w[4] >> 32) as u32,
                );
                let fmt = vertex_format(vfmt).ok_or(format!("vertex format {vfmt}"))?;
                let vdev = dev_addr(g, sub, va, vcount as u64 * stride)
                    .ok_or("vertices in no memory pool")?;
                let (it, idev) = if icount != 0 {
                    let (t, sz) = match ity {
                        1 => (vk::IndexType::UINT16, 2),
                        2 => (vk::IndexType::UINT32, 4),
                        _ => return Err(format!("index type {ity}")),
                    };
                    (
                        t,
                        dev_addr(g, sub, ia, icount as u64 * sz)
                            .ok_or("indices in no memory pool")?,
                    )
                } else {
                    (vk::IndexType::NONE_KHR, 0)
                };
                let tri = vk::AccelerationStructureGeometryTrianglesDataKHR::default()
                    .vertex_format(fmt)
                    .vertex_data(vk::DeviceOrHostAddressConstKHR {
                        device_address: vdev,
                    })
                    .vertex_stride(stride)
                    .max_vertex(vcount.saturating_sub(1))
                    .index_type(it)
                    .index_data(vk::DeviceOrHostAddressConstKHR {
                        device_address: idev,
                    });
                vk_geos.push(
                    vk::AccelerationStructureGeometryKHR::default()
                        .geometry_type(vk::GeometryTypeKHR::TRIANGLES)
                        .geometry(vk::AccelerationStructureGeometryDataKHR { triangles: tri })
                        .flags(vk::GeometryFlagsKHR::OPAQUE),
                );
                prims.push(if icount != 0 { icount / 3 } else { vcount / 3 });
            }
            2 => {
                let (ba, count, stride) = (
                    ge.w[0],
                    (ge.w[1] & 0xffff_ffff) as u32,
                    (ge.w[1] >> 32).max(24),
                );
                let bdev =
                    dev_addr(g, sub, ba, count as u64 * stride).ok_or("boxes in no memory pool")?;
                let aabb = vk::AccelerationStructureGeometryAabbsDataKHR::default()
                    .data(vk::DeviceOrHostAddressConstKHR {
                        device_address: bdev,
                    })
                    .stride(stride);
                vk_geos.push(
                    vk::AccelerationStructureGeometryKHR::default()
                        .geometry_type(vk::GeometryTypeKHR::AABBS)
                        .geometry(vk::AccelerationStructureGeometryDataKHR { aabbs: aabb }),
                );
                prims.push(count);
            }
            3 => {
                // instance records (VkAccelerationStructureInstanceKHR's 64 bytes): the bottom level named by its guest
                // address is patched to the host structure's device address
                let (ia, count) = (ge.w[0], (ge.w[1] & 0xffff_ffff) as u32);
                let n = count as u64 * 64;
                let src = super::mem::guest_bytes(ia, n);
                let ib = g.buffer(
                    n.max(64),
                    vk::BufferUsageFlags::ACCELERATION_STRUCTURE_BUILD_INPUT_READ_ONLY_KHR
                        | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS,
                    MemoryLocation::CpuToGpu,
                    "rt instances",
                )?;
                let mut recs = src.to_vec();
                for k in 0..count as usize {
                    let r = &mut recs[k * 64 + 56..k * 64 + 64];
                    let guest = u64::from_le_bytes(r.try_into().unwrap());
                    let host = lookup(guest)
                        .map(|x| x.1)
                        .ok_or(format!("instance {k} names {guest:#x}, built nowhere"))?;
                    r.copy_from_slice(&host.to_le_bytes());
                }
                unsafe { core::ptr::copy_nonoverlapping(recs.as_ptr(), ib.ptr, recs.len()) };
                let inst = vk::AccelerationStructureGeometryInstancesDataKHR::default().data(
                    vk::DeviceOrHostAddressConstKHR {
                        device_address: ib.addr,
                    },
                );
                vk_geos.push(
                    vk::AccelerationStructureGeometryKHR::default()
                        .geometry_type(vk::GeometryTypeKHR::INSTANCES)
                        .geometry(vk::AccelerationStructureGeometryDataKHR { instances: inst }),
                );
                prims.push(count);
                keep.push(ib);
            }
            k => return Err(format!("geometry kind {k}")),
        }
    }
    let ty = if top {
        vk::AccelerationStructureTypeKHR::TOP_LEVEL
    } else {
        vk::AccelerationStructureTypeKHR::BOTTOM_LEVEL
    };
    let flags = vk::BuildAccelerationStructureFlagsKHR::PREFER_FAST_TRACE
        | if b.flags & crate::nvn::query::AS_ALLOW_UPDATE != 0 {
            vk::BuildAccelerationStructureFlagsKHR::ALLOW_UPDATE
        } else {
            vk::BuildAccelerationStructureFlagsKHR::empty()
        };
    let mut info = vk::AccelerationStructureBuildGeometryInfoKHR::default()
        .ty(ty)
        .flags(flags)
        .mode(vk::BuildAccelerationStructureModeKHR::BUILD)
        .geometries(&vk_geos);
    let mut sizes = vk::AccelerationStructureBuildSizesInfoKHR::default();
    unsafe {
        g.d.accel.get_acceleration_structure_build_sizes(
            vk::AccelerationStructureBuildTypeKHR::DEVICE,
            &info,
            &prims,
            &mut sizes,
        )
    };
    let buf = g.buffer(
        sizes.acceleration_structure_size,
        vk::BufferUsageFlags::ACCELERATION_STRUCTURE_STORAGE_KHR
            | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS,
        MemoryLocation::GpuOnly,
        "rt structure",
    )?;
    let ci = vk::AccelerationStructureCreateInfoKHR::default()
        .buffer(buf.buf)
        .size(sizes.acceleration_structure_size)
        .ty(ty);
    let handle = unsafe { g.d.accel.create_acceleration_structure(&ci, None) }
        .map_err(|e| format!("vkCreateAccelerationStructureKHR: {e}"))?;
    let scratch = g.buffer(
        sizes.build_scratch_size + 256,
        vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS,
        MemoryLocation::GpuOnly,
        "rt scratch",
    )?;
    info = info
        .dst_acceleration_structure(handle)
        .scratch_data(vk::DeviceOrHostAddressKHR {
            device_address: (scratch.addr + 255) & !255,
        });
    let ranges: Vec<vk::AccelerationStructureBuildRangeInfoKHR> = prims
        .iter()
        .map(|&p| vk::AccelerationStructureBuildRangeInfoKHR::default().primitive_count(p))
        .collect();
    super::queue::full_barrier(g, sub.main);
    unsafe {
        g.d.accel
            .cmd_build_acceleration_structures(sub.main, &[info], &[&ranges])
    };
    super::queue::full_barrier(g, sub.main);
    let addr = unsafe {
        g.d.accel.get_acceleration_structure_device_address(
            &vk::AccelerationStructureDeviceAddressInfoKHR::default()
                .acceleration_structure(handle),
        )
    };
    sub.transient.push(scratch);
    sub.transient.extend(keep);
    let old = ACCELS
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .insert(
            b.destination,
            Accel {
                handle,
                buf,
                addr,
                top,
            },
        );
    if let Some(o) = old {
        g.q.defer_after_use(Box::new(move |g: &Gpu| unsafe {
            g.d.accel.destroy_acceleration_structure(o.handle, None);
            g.free_buffer(o.buf);
        }));
    }
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    if N.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 8 {
        crate::vklog!(
            "ray tracing: {} level built at {:#x} ({} geometries, {} primitives, {} KB)",
            if top { "top" } else { "bottom" },
            b.destination,
            geos.len(),
            prims.iter().sum::<u32>(),
            sizes.acceleration_structure_size >> 10
        );
    }
    Ok(())
}
