//! What the GPU tests share: a device brought up through the NVN API the way a game does it, guest-like memory (with
//! the OS write watch, as the runtime's guest memory has), programs made from the test shaders, and image checks.
#![allow(dead_code, clippy::missing_safety_doc)]


use nxgfx::nvn::types::*;
use nxgfx::nvn::{cmd::*, device::*, pool::*, prog::*, queue::*, state::*, sync::*, tex::*};
use std::sync::{Mutex, MutexGuard, Once};

static LOCK: Mutex<()> = Mutex::new(());
static INIT: Once = Once::new();

/// Serializes the GPU tests (they share the process's device) and turns the validation layer on.
pub fn setup() -> MutexGuard<'static, ()> {
    INIT.call_once(|| {
        if std::env::var("NXGFX_VALIDATION").is_err() {
            std::env::set_var("NXGFX_VALIDATION", "1");
        }
        // overlays and capture tools of the machine stay out of the tests
        std::env::set_var("VK_LOADER_LAYERS_DISABLE", "~implicit~");
    });
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Memory as the game's: committed pages with the OS write watch.
pub struct GuestMem {
    pub ptr: *mut u8,
    pub size: usize,
}
unsafe impl Send for GuestMem {}
impl GuestMem {
    pub fn new(size: usize) -> GuestMem {
        use windows_sys::Win32::System::Memory::{VirtualAlloc, MEM_COMMIT, MEM_RESERVE, PAGE_READWRITE};
        const MEM_WRITE_WATCH: u32 = 0x200000;
        let size = (size + 0xffff) & !0xffff;
        let p = unsafe { VirtualAlloc(core::ptr::null(), size, MEM_RESERVE | MEM_COMMIT | MEM_WRITE_WATCH, PAGE_READWRITE) } as *mut u8;
        assert!(!p.is_null(), "VirtualAlloc");
        GuestMem { ptr: p, size }
    }
    pub fn addr(&self) -> u64 {
        self.ptr as u64
    }
    pub fn slice(&self) -> &mut [u8] {
        unsafe { core::slice::from_raw_parts_mut(self.ptr, self.size) }
    }
}
impl Drop for GuestMem {
    fn drop(&mut self) {
        use windows_sys::Win32::System::Memory::{VirtualFree, MEM_RELEASE};
        unsafe { VirtualFree(self.ptr as *mut _, 0, MEM_RELEASE) };
    }
}

/// A memory pool over guest memory, with a bump allocator.
pub struct Pool {
    pub obj: Box<NVNmemoryPool>,
    pub mem: GuestMem,
    pub next: usize,
}
impl Pool {
    pub fn new(dev: *mut NVNdevice, size: usize, flags: u32) -> Pool {
        let mem = GuestMem::new(size);
        let mut b = NVNmemoryPoolBuilder::boxed();
        let mut obj = NVNmemoryPool::boxed();
        unsafe {
            nvnMemoryPoolBuilderSetDefaults(&mut *b);
            nvnMemoryPoolBuilderSetDevice(&mut *b, dev);
            nvnMemoryPoolBuilderSetStorage(&mut *b, mem.ptr, mem.size);
            nvnMemoryPoolBuilderSetFlags(&mut *b, flags as i32);
            assert_eq!(nvnMemoryPoolInitialize(&mut *obj, &*b), 1);
        }
        Pool { obj, mem, next: 0 }
    }
    pub fn alloc(&mut self, n: usize, align: usize) -> (usize, u64) {
        let off = (self.next + align - 1) & !(align - 1);
        self.next = off + n;
        assert!(self.next <= self.mem.size, "test pool full");
        (off, self.mem.addr() + off as u64)
    }
    pub fn put<T: Copy>(&mut self, data: &[T], align: usize) -> u64 {
        let n = core::mem::size_of_val(data);
        let (off, a) = self.alloc(n, align);
        unsafe { core::ptr::copy_nonoverlapping(data.as_ptr() as *const u8, self.mem.ptr.add(off), n) };
        a
    }
    pub fn p(&mut self) -> *mut NVNmemoryPool {
        &mut *self.obj
    }
}
impl Drop for Pool {
    fn drop(&mut self) {
        unsafe { nvnMemoryPoolFinalize(&mut *self.obj) };
    }
}

pub const CPU_POOL: u32 = mem_flags::CPU_UNCACHED | mem_flags::GPU_CACHED;
pub const GPU_POOL: u32 = mem_flags::CPU_NO_ACCESS | mem_flags::GPU_CACHED;

/// Device, queue, a command buffer with the game's memory, descriptor pools: what every test starts from.
pub struct Nvn {
    pub dev: Box<NVNdevice>,
    pub queue: Box<NVNqueue>,
    pub qmem: GuestMem,
    pub cmd: Box<NVNcommandBuffer>,
    pub cmd_pool: Pool,
    pub ctl: Vec<u64>,
    pub data: Pool,
    pub gpu: Pool,
    pub code: Pool,
    pub tex_pool: Box<NVNtexturePool>,
    pub samp_pool: Box<NVNsamplerPool>,
    pub desc: Pool,
    pub sync: Box<NVNsync>,
}

impl Nvn {
    pub fn new() -> Nvn {
        Self::with_origin(origin::LOWER_LEFT)
    }
    pub fn with_origin(org: u32) -> Nvn {
        let mut dev = NVNdevice::boxed();
        let mut db = NVNdeviceBuilder::boxed();
        unsafe {
            nvnDeviceBuilderSetDefaults(&mut *db);
            assert_eq!(nvnDeviceInitialize(&mut *dev, &*db), 1, "device");
            nvnDeviceSetWindowOriginMode(&mut *dev, org);
        }
        let d: *mut NVNdevice = &mut *dev;
        let mut qb = NVNqueueBuilder::boxed();
        let mut queue = NVNqueue::boxed();
        let qmem;
        unsafe {
            nvnQueueBuilderSetDevice(&mut *qb, d);
            nvnQueueBuilderSetDefaults(&mut *qb);
            let n = nvnQueueBuilderGetQueueMemorySize(&*qb);
            qmem = GuestMem::new(n);
            nvnQueueBuilderSetQueueMemory(&mut *qb, qmem.ptr, qmem.size);
            assert_eq!(nvnQueueInitialize(&mut *queue, &*qb), 1);
        }
        let mut cmd_pool = Pool::new(d, 1 << 20, CPU_POOL);
        let mut cmd = NVNcommandBuffer::boxed();
        let mut ctl = vec![0u64; 8192];
        unsafe {
            nvnCommandBufferInitialize(&mut *cmd, d);
            nvnCommandBufferAddCommandMemory(&mut *cmd, cmd_pool.p(), 0, 1 << 20);
            nvnCommandBufferAddControlMemory(&mut *cmd, ctl.as_mut_ptr() as *mut u8, ctl.len() * 8);
        }
        let data = Pool::new(d, 16 << 20, CPU_POOL);
        let gpu = Pool::new(d, 32 << 20, GPU_POOL);
        let code = Pool::new(d, 4 << 20, mem_flags::CPU_UNCACHED | mem_flags::GPU_CACHED | mem_flags::SHADER_CODE);
        let mut desc = Pool::new(d, 1 << 20, CPU_POOL);
        let mut tex_pool = NVNtexturePool::boxed();
        let mut samp_pool = NVNsamplerPool::boxed();
        unsafe {
            assert_eq!(nvnTexturePoolInitialize(&mut *tex_pool, desc.p(), 0, 256), 1);
            assert_eq!(nvnSamplerPoolInitialize(&mut *samp_pool, desc.p(), 256 * 32, 64), 1);
        }
        let mut sync = NVNsync::boxed();
        unsafe { nvnSyncInitialize(&mut *sync, d) };
        Nvn { dev, queue, qmem, cmd, cmd_pool, ctl, data, gpu, code, tex_pool, samp_pool, desc, sync }
    }

    pub fn d(&mut self) -> *mut NVNdevice {
        &mut *self.dev
    }
    pub fn c(&mut self) -> *mut NVNcommandBuffer {
        &mut *self.cmd
    }

    pub fn begin(&mut self) {
        unsafe {
            nvnCommandBufferBeginRecording(self.c());
            nvnCommandBufferSetTexturePool(self.c(), &*self.tex_pool);
            nvnCommandBufferSetSamplerPool(self.c(), &*self.samp_pool);
        }
    }
    /// Ends the recording, submits it, fences and waits: everything it did is visible to the CPU after this.
    pub fn submit_wait(&mut self) {
        unsafe {
            let h = nvnCommandBufferEndRecording(self.c());
            nvnQueueSubmitCommands(&mut *self.queue, 1, &h);
            nvnQueueFenceSync(&mut *self.queue, &mut *self.sync, 0, 0);
            nvnQueueFlush(&mut *self.queue);
            let r = nvnSyncWait(&*self.sync, 10_000_000_000);
            assert!(r == sync_wait::ALREADY_SIGNALED || r == sync_wait::CONDITION_SATISFIED, "sync wait {r}");
        }
        // the next recording starts at the start of the command memory again, as a game's ring does
        unsafe { nvnCommandBufferAddCommandMemory(self.c(), self.cmd_pool.p(), 0, 1 << 20) };
    }

    /// A texture in `pool` (GPU-only pool when None).
    pub fn texture(&mut self, w: u32, h: u32, format: i32, flags: u32, cpu_pool: bool) -> Box<NVNtexture> {
        let d = self.d();
        let mut b = NVNtextureBuilder::boxed();
        let mut t = NVNtexture::boxed();
        unsafe {
            nvnTextureBuilderSetDevice(&mut *b, d);
            nvnTextureBuilderSetDefaults(&mut *b);
            nvnTextureBuilderSetSize2D(&mut *b, w as i32, h as i32);
            nvnTextureBuilderSetFormat(&mut *b, format);
            nvnTextureBuilderSetFlags(&mut *b, flags as i32);
            let size = nvnTextureBuilderGetStorageSize(&*b);
            let align = nvnTextureBuilderGetStorageAlignment(&*b);
            let pool = if cpu_pool { &mut self.data } else { &mut self.gpu };
            let (off, _) = pool.alloc(size, align);
            nvnTextureBuilderSetStorage(&mut *b, pool.p(), off as isize);
            assert_eq!(nvnTextureInitialize(&mut *t, &*b), 1);
        }
        t
    }

    /// Reads a 2D RGBA8 texture back (nvnTextureReadTexels).
    pub fn read_rgba8(&mut self, t: &NVNtexture, w: u32, h: u32) -> Vec<u8> {
        let mut v = vec![0u8; (w * h * 4) as usize];
        let r = NVNcopyRegion { xoffset: 0, yoffset: 0, zoffset: 0, width: w as i32, height: h as i32, depth: 1 };
        unsafe { nvnTextureReadTexels(t, core::ptr::null(), &r, v.as_mut_ptr()) };
        v
    }

    pub fn register_texture(&mut self, id: i32, t: &NVNtexture) {
        unsafe { nvnTexturePoolRegisterTexture(&*self.tex_pool, id, t, core::ptr::null()) };
    }
    pub fn register_sampler(&mut self, id: i32, nearest: bool) {
        let mut b = NVNsamplerBuilder::boxed();
        let mut s = NVNsampler::boxed();
        unsafe {
            nvnSamplerBuilderSetDevice(&mut *b, self.d());
            nvnSamplerBuilderSetDefaults(&mut *b);
            let f = if nearest { 0 } else { 1 };
            nvnSamplerBuilderSetMinMagFilter(&mut *b, f, f);
            nvnSamplerBuilderSetWrapMode(&mut *b, wrap::CLAMP_TO_EDGE as i32, wrap::CLAMP_TO_EDGE as i32, wrap::CLAMP_TO_EDGE as i32);
            nvnSamplerInitialize(&mut *s, &*b);
            nvnSamplerPoolRegisterSampler(&*self.samp_pool, id, &*s);
        }
    }

    /// A program from test shaders: each stage's SPIR-V goes into the shader library under the key its fabricated
    /// control section and code area hash to (as lane C's build keys a game's shaders), with its reflection json.
    pub fn program(&mut self, stages: &[(&[u8], &str)]) -> Box<NVNprogram> {
        let g = nxgfx::vk::Gpu::get().unwrap();
        let mut sds = Vec::new();
        for (i, (spv, json)) in stages.iter().enumerate() {
            // code area: the SPIR-V bytes plus a tag (unique per stage), in a shader code pool
            let mut code = spv.to_vec();
            code.extend_from_slice(&(i as u32).to_le_bytes());
            let addr = self.code.put(&code, 256);
            let mut ctl = vec![0u8; 0x880];
            ctl[0x20..0x24].copy_from_slice(&0x880u32.to_le_bytes());
            ctl[0x704..0x708].copy_from_slice(&(code.len() as u32).to_le_bytes());
            let ctl: &'static [u8] = Box::leak(ctl.into_boxed_slice());
            let sd = NVNshaderData { data: addr, control: ctl.as_ptr() };
            let key = unsafe { shader_key(&sd, true) }.expect("shader key");
            let words: Vec<u32> = spv.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
            g.shaders.insert(key, words, Some(json.to_string()));
            sds.push(sd);
        }
        let mut p = NVNprogram::boxed();
        unsafe {
            nvnProgramInitialize(&mut *p, self.d());
            assert_eq!(nvnProgramSetShaders(&mut *p, sds.len() as i32, sds.as_ptr()), 1);
        }
        p
    }

    /// Render target, viewport, scissor and default states for a w x h pass.
    pub fn set_target(&mut self, color: &NVNtexture, depth: Option<&NVNtexture>, w: u32, h: u32) {
        let c = self.c();
        let cts = [color as *const NVNtexture];
        unsafe {
            nvnCommandBufferSetRenderTargets(c, 1, cts.as_ptr(), core::ptr::null(), depth.map(|d| d as *const NVNtexture).unwrap_or(core::ptr::null()), core::ptr::null());
            nvnCommandBufferSetViewport(c, 0, 0, w as i32, h as i32);
            nvnCommandBufferSetScissor(c, 0, 0, w as i32, h as i32);
            let mut cs = NVNcolorState::boxed();
            nvnColorStateSetDefaults(&mut *cs);
            nvnCommandBufferBindColorState(c, &*cs);
            let mut bs = NVNblendState::boxed();
            nvnBlendStateSetDefaults(&mut *bs);
            nvnCommandBufferBindBlendState(c, &*bs);
            let mut cm = NVNchannelMaskState::boxed();
            nvnChannelMaskStateSetDefaults(&mut *cm);
            nvnCommandBufferBindChannelMaskState(c, &*cm);
            let mut ps = NVNpolygonState::boxed();
            nvnPolygonStateSetDefaults(&mut *ps);
            nvnCommandBufferBindPolygonState(c, &*ps);
            let mut ds = NVNdepthStencilState::boxed();
            nvnDepthStencilStateSetDefaults(&mut *ds);
            nvnCommandBufferBindDepthStencilState(c, &*ds);
            let mut ms = NVNmultisampleState::boxed();
            nvnMultisampleStateSetDefaults(&mut *ms);
            nvnCommandBufferBindMultisampleState(c, &*ms);
        }
    }

    /// Vertex layout: attributes (format, offset) on stream 0 of `stride` bytes.
    pub fn vertex_layout(&mut self, attrs: &[(i32, u32)], stride: u32) {
        let c = self.c();
        let mut va: Vec<Box<NVNvertexAttribState>> = Vec::new();
        for &(f, off) in attrs {
            let mut a = NVNvertexAttribState::boxed();
            unsafe {
                nvnVertexAttribStateSetDefaults(&mut *a);
                nvnVertexAttribStateSetFormat(&mut *a, f, off as isize);
                nvnVertexAttribStateSetStreamIndex(&mut *a, 0);
            }
            va.push(a);
        }
        let flat: Vec<u32> = va.iter().map(|a| u32::from_le_bytes(a.0)).collect();
        let mut s = NVNvertexStreamState::boxed();
        unsafe {
            nvnVertexStreamStateSetDefaults(&mut *s);
            nvnVertexStreamStateSetStride(&mut *s, stride as isize);
            nvnCommandBufferBindVertexAttribState(c, flat.len() as i32, flat.as_ptr() as *const NVNvertexAttribState);
            nvnCommandBufferBindVertexStreamState(c, 1, &*s);
        }
    }

    pub fn finish(&mut self) {
        unsafe { nvnQueueFinish(&mut *self.queue) };
    }
}

impl Drop for Nvn {
    fn drop(&mut self) {
        self.finish();
        unsafe {
            nvnSyncFinalize(&mut *self.sync);
            nvnCommandBufferFinalize(self.c());
            nvnQueueFinalize(&mut *self.queue);
            nvnDeviceFinalize(self.d());
        }
    }
}

pub fn spv(name: &str) -> Vec<u8> {
    let p = std::path::Path::new(env!("OUT_DIR")).join("test_spv").join(format!("{name}.spv"));
    std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// Validation errors since the process started.
pub fn validation_errors() -> u64 {
    nxgfx::vk::validation_counts().0
}

/// Expected coverage of a triangle (NDC vertices) at pixel centres of a w x h target, in memory rows. `flip`: the
/// upper-left origin (NDC +y is row 0); otherwise NDC +y is the last row (NVN's lower-left origin). Pixels within one
/// pixel of an edge are None (rasterization rules decide them).
pub fn tri_cover(v: [[f32; 2]; 3], w: u32, h: u32, flip: bool) -> Vec<Option<bool>> {
    let win: Vec<(f64, f64)> = v
        .iter()
        .map(|p| {
            let x = (p[0] as f64 + 1.0) / 2.0 * w as f64;
            let y = if flip { (1.0 - p[1] as f64) / 2.0 * h as f64 } else { (p[1] as f64 + 1.0) / 2.0 * h as f64 };
            (x, y)
        })
        .collect();
    let edge = |a: (f64, f64), b: (f64, f64), p: (f64, f64)| -> f64 {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        ((p.0 - a.0) * dy - (p.1 - a.1) * dx) / (dx * dx + dy * dy).sqrt()
    };
    let area = edge(win[0], win[1], win[2]);
    let mut out = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        for x in 0..w {
            let p = (x as f64 + 0.5, y as f64 + 0.5);
            let e = [edge(win[0], win[1], p), edge(win[1], win[2], p), edge(win[2], win[0], p)];
            let s = if area > 0.0 { 1.0 } else { -1.0 };
            let d: Vec<f64> = e.iter().map(|x| x * s).collect();
            if d.iter().all(|&x| x > 1.0) {
                out.push(Some(true));
            } else if d.iter().any(|&x| x < -1.0) {
                out.push(Some(false));
            } else {
                out.push(None);
            }
        }
    }
    out
}

/// Pixels of `got` that differ from `want` by more than `tol` in any channel; `None` in `want` = don't care.
pub fn diff(got: &[u8], want: &[Option<[u8; 4]>], tol: u8) -> Vec<(usize, [u8; 4], [u8; 4])> {
    let mut bad = Vec::new();
    for (i, w) in want.iter().enumerate() {
        if let Some(w) = w {
            let g = [got[4 * i], got[4 * i + 1], got[4 * i + 2], got[4 * i + 3]];
            if (0..4).any(|k| g[k].abs_diff(w[k]) > tol) {
                bad.push((i, g, *w));
            }
        }
    }
    bad
}

pub fn json(stage: &str, bindings: &str) -> String {
    format!(r#"{{"layout": "nxshaders 1", "stage": "{stage}", "status": "ok", "bindings": [{bindings}], "c1": ""}}"#)
}
