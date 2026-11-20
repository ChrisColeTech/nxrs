//! The Vulkan back end: replays the record layer's command lists on the GPU.
//!
//! | Module | Part |
//! |---|---|
//! | `device` | instance, device, features, validation counters |
//! | `layout` | the one pipeline layout every program shares (per-stage sets + the pool set) |
//! | `tables` | the pool descriptor set: texture, image and sampler IDs as bindless arrays |
//! | `mem` | memory pools on a discrete GPU: placement, CPU-write upload, GPU-write write-back |
//! | `image` | textures as images: creation, views, uploads from storage, texel reads and writes |
//! | `shader` | the shader library (SPIR-V by shader key) and its modules |
//! | `pipe` | pipelines by key, the key file and the memory-budgeted prewarm |
//! | `exec` | the replay of one submission's records |
//! | `queue` | submissions, the timeline, completion and its CPU-visible effects, queue parking, presents |
//! | `present` | the host window's swap chain and the pacing log |

pub mod counters;
pub mod device;
pub mod exec;
pub mod framegen;
pub mod gmem;
pub mod image;
pub mod layout;
pub mod lazy;
pub mod live;
pub mod lsfg;
pub mod mem;
pub mod pcache;
pub mod perf;
pub mod pipe;
pub mod present;
pub mod queue;
pub mod scale;
pub mod spvfix;
pub mod upscale;
pub mod shader;
pub mod shot;
pub mod sparse;
pub mod tables;
pub mod xfb;

use ash::vk;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme, Allocator, AllocatorCreateDesc};
use gpu_allocator::MemoryLocation;
use std::sync::{Mutex, OnceLock};

pub use device::{VALIDATION_ERRORS, VALIDATION_WARNINGS};

/// A buffer with its memory; `ptr` is its mapping when host-visible.
pub struct Buf {
    pub buf: vk::Buffer,
    pub alloc: Option<Allocation>,
    /// memory of an imported host range (no allocator)
    pub mem: vk::DeviceMemory,
    pub size: u64,
    pub ptr: *mut u8,
    /// the buffer's device address (0: none; descriptor buffers point at buffers by address)
    pub addr: u64,
}
unsafe impl Send for Buf {}
unsafe impl Sync for Buf {}

pub struct Gpu {
    pub d: device::Device,
    pub alloc: Mutex<Allocator>,
    pub layout: layout::Layout,
    pub tables: tables::Tables,
    pub mem: Mutex<mem::Pools>,
    pub images: Mutex<image::Images>,
    pub shaders: shader::Library,
    pub pipes: pipe::Pipes,
    pub q: queue::Queue,
    pub present: Mutex<present::Presenter>,
    pub astc: Mutex<Option<crate::tex::astc::Astc>>,
}

// The back end's state is behind its own locks; handles and game-memory addresses in it are plain values.
unsafe impl Send for Gpu {}
unsafe impl Sync for Gpu {}

static GPU: OnceLock<Result<&'static Gpu, String>> = OnceLock::new();

impl Gpu {
    /// The back end, created at the first call (nvnDeviceInitialize).
    pub fn get() -> Result<&'static Gpu, String> {
        GPU.get_or_init(|| Gpu::create().map(|g| &*Box::leak(Box::new(g)))).clone()
    }
    /// Runs `f` on the back end if it exists (a red line otherwise).
    pub fn with<R: Default>(f: impl FnOnce(&'static Gpu) -> R) -> R {
        match Gpu::get() {
            Ok(g) => f(g),
            Err(e) => {
                crate::log::red_once(&format!("no Vulkan device: {e}"));
                R::default()
            }
        }
    }
    pub fn try_get() -> Option<&'static Gpu> {
        GPU.get().and_then(|r| r.as_ref().ok().copied())
    }

    fn create() -> Result<Gpu, String> {
        let d = device::Device::new()?;
        let alloc = Allocator::new(&AllocatorCreateDesc {
            instance: d.instance.clone(),
            device: d.dev.clone(),
            physical_device: d.pdev,
            debug_settings: Default::default(),
            buffer_device_address: d.caps.bda,
            allocation_sizes: Default::default(),
        })
        .map_err(|e| format!("allocator: {e}"))?;
        let layout = layout::Layout::new(&d)?;
        let tables = tables::Tables::new(&d, &layout)?;
        let q = queue::Queue::new(&d)?;
        let pipes = pipe::Pipes::new(&d);
        let g = Gpu {
            alloc: Mutex::new(alloc),
            layout,
            tables,
            mem: Mutex::new(mem::Pools::default()),
            images: Mutex::new(image::Images::default()),
            shaders: shader::Library::new(),
            pipes,
            q,
            present: Mutex::new(present::Presenter::default()),
            astc: Mutex::new(None),
            d,
        };
        Ok(g)
    }

    pub fn dev(&self) -> &ash::Device {
        &self.d.dev
    }

    /// A buffer in `loc` memory (mapped when host-visible).
    pub fn buffer(&self, size: u64, usage: vk::BufferUsageFlags, loc: MemoryLocation, name: &str) -> Result<Buf, String> {
        let size = size.max(16);
        // what this layer takes from system memory is capped (Turbo 8a4618c): 8 GB or a quarter of the machine's
        // memory, NXGFX_HOST_MB (0 = no cap); over it the allocation is refused and its caller's fallback applies
        if loc != MemoryLocation::GpuOnly {
            let cap = host_cap();
            let live = HOST_LIVE.load(std::sync::atomic::Ordering::Relaxed);
            if live + size > cap {
                let n = HOST_REFUSED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if (n & n.wrapping_sub(1)) == 0 {
                    crate::log::red(&format!("system memory cap: {name} of {} MB refused, {} MB of this layer's in system memory, cap {} MB (NXGFX_HOST_MB); {} refused so far", size >> 20, live >> 20, cap >> 20, n + 1));
                }
                return Err(format!("{name}: over the system memory cap"));
            }
        }
        // buffers a shader reads get a device address (descriptor buffers name them by it)
        let bound = vk::BufferUsageFlags::UNIFORM_BUFFER | vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::UNIFORM_TEXEL_BUFFER | vk::BufferUsageFlags::STORAGE_TEXEL_BUFFER;
        let usage = if self.d.caps.bda && usage.intersects(bound) { usage | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS } else { usage };
        let ci = vk::BufferCreateInfo::default().size(size).usage(usage).sharing_mode(vk::SharingMode::EXCLUSIVE);
        let buf = unsafe { self.dev().create_buffer(&ci, None) }.map_err(|e| format!("vkCreateBuffer {name}: {e}"))?;
        let req = unsafe { self.dev().get_buffer_memory_requirements(buf) };
        let a = self
            .alloc
            .lock()
            .unwrap()
            .allocate(&AllocationCreateDesc { name, requirements: req, location: loc, linear: true, allocation_scheme: AllocationScheme::GpuAllocatorManaged })
            .map_err(|e| format!("allocation {name} ({size} bytes): {e}"))?;
        unsafe { self.dev().bind_buffer_memory(buf, a.memory(), a.offset()) }.map_err(|e| format!("bind {name}: {e}"))?;
        let ptr = a.mapped_ptr().map(|p| p.as_ptr() as *mut u8).unwrap_or(core::ptr::null_mut());
        if !ptr.is_null() {
            HOST_LIVE.fetch_add(a.size(), std::sync::atomic::Ordering::Relaxed);
        }
        let addr = if usage.contains(vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS) { unsafe { self.dev().get_buffer_device_address(&vk::BufferDeviceAddressInfo::default().buffer(buf)) } } else { 0 };
        Ok(Buf { buf, alloc: Some(a), mem: vk::DeviceMemory::null(), size, ptr, addr })
    }

    pub fn free_buffer(&self, b: Buf) {
        unsafe { self.dev().destroy_buffer(b.buf, None) };
        if let Some(a) = b.alloc {
            if a.mapped_ptr().is_some() {
                HOST_LIVE.fetch_sub(a.size(), std::sync::atomic::Ordering::Relaxed);
            }
            let _ = self.alloc.lock().unwrap().free(a);
        }
        if b.mem != vk::DeviceMemory::null() {
            unsafe { self.dev().free_memory(b.mem, None) };
        }
    }

    /// nvnProgramFinalize: the replay resolves a program's modules when it is bound and keeps no pointer to it.
    pub fn prog_forget(&self, _serial: u64) {}

    pub fn timestamp_period(&self) -> f32 {
        self.d.caps.timestamp_period
    }
}

/// Bytes of host-visible (system) memory this layer's buffers hold, and allocations refused over the cap.
pub static HOST_LIVE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static HOST_REFUSED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn host_cap() -> u64 {
    static V: OnceLock<u64> = OnceLock::new();
    *V.get_or_init(|| {
        if let Some(mb) = std::env::var("NXGFX_HOST_MB").ok().and_then(|v| v.parse::<u64>().ok()) {
            return if mb == 0 { u64::MAX } else { mb << 20 };
        }
        let total = total_phys();
        (8u64 << 30).min(if total > 0 { total / 4 } else { 8 << 30 })
    })
}

#[cfg(windows)]
fn total_phys() -> u64 {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut m: MEMORYSTATUSEX = unsafe { core::mem::zeroed() };
    m.dwLength = core::mem::size_of::<MEMORYSTATUSEX>() as u32;
    if unsafe { GlobalMemoryStatusEx(&mut m) } != 0 { m.ullTotalPhys } else { 0 }
}
#[cfg(not(windows))]
fn total_phys() -> u64 {
    0
}

/// Validation messages counted since the process started (errors, warnings).
pub fn validation_counts() -> (u64, u64) {
    use std::sync::atomic::Ordering;
    (VALIDATION_ERRORS.load(Ordering::Relaxed), VALIDATION_WARNINGS.load(Ordering::Relaxed))
}
