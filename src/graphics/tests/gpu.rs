//! Headless rendering through the NVN API, each frame compared with the image it must give, with the Vulkan
//! validation layer on and no message allowed. `cargo test -p graphics --features gpu-tests -- --test-threads=1`.
#![cfg(feature = "gpu-tests")]

mod common;
use common::*;
use nxgfx::nvn::cmd::*;

use nxgfx::nvn::state::*;
use nxgfx::nvn::types::*;
use nxgfx::nvn::tex::*;

const W: u32 = 64;
const H: u32 = 64;

fn tri_program(n: &mut Nvn) -> Box<NVNprogram> {
    let vs = spv("tri_vert");
    let fs = spv("tri_frag");
    n.program(&[(&vs, &json("vs", "")), (&fs, &json("ps", ""))])
}

fn quad_program(n: &mut Nvn) -> Box<NVNprogram> {
    let vs = spv("quad_vert");
    let fs = spv("quad_frag");
    let fj = json(
        "ps",
        r#"{"set": 0, "binding": 0, "type": "uniform_buffer", "count": 1, "name": "support_buffer", "resource": {"kind": "support"}},
           {"set": 1, "binding": 1, "type": "combined_image_sampler", "count": 1, "name": "fp_t_tcb_8",
            "resource": {"kind": "texture", "type": 3, "format": "Unknown", "cbuf_slot": -1, "handle": 8, "array": 1, "separate": false, "flags": 0, "slot": 0}},
           {"set": 1, "binding": 2, "type": "uniform_buffer", "count": 1, "name": "fp_c3", "resource": {"kind": "cbuf", "slot": 3, "fixed": 0}}"#,
    );
    n.program(&[(&vs, &json("vs", "")), (&fs, &fj)])
}

fn clear(n: &mut Nvn, rgba: [f32; 4]) {
    unsafe { nvnCommandBufferClearColor(n.c(), 0, rgba.as_ptr(), 0xf) };
}

fn bind_program(n: &mut Nvn, p: &NVNprogram) {
    unsafe { nvnCommandBufferBindProgram(n.c(), p, 0x1f) };
}

/// Draws a triangle: vertices (x, y, z, rgba) in a CPU-visible pool.
fn draw_tri(n: &mut Nvn, v: [[f32; 3]; 3], rgba: [f32; 4]) {
    let mut data = Vec::new();
    for p in v {
        data.extend_from_slice(&p);
        data.extend_from_slice(&rgba);
    }
    let a = n.data.put(&data, 256);
    n.vertex_layout(&[(f::RGB32F, 0), (f::RGBA32F, 12)], 28);
    unsafe {
        nvnCommandBufferBindVertexBuffer(n.c(), 0, a, data.len() * 4);
        nvnCommandBufferDrawArrays(n.c(), prim::TRIANGLES, 0, 3);
    }
}

mod f {
    pub const RGBA8: i32 = 37;
    pub const RGB32F: i32 = 34;
    pub const RGBA32F: i32 = 46;
    pub const RG32F: i32 = 22;
}

fn check(name: &str, got: &[u8], want: &[Option<[u8; 4]>], tol: u8) {
    let bad = diff(got, want, tol);
    let checked = want.iter().filter(|w| w.is_some()).count();
    eprintln!("{name}: {checked} pixels compared, {} differ", bad.len());
    assert!(bad.is_empty(), "{name}: {} of {checked} pixels differ, first {:?}", bad.len(), &bad[..bad.len().min(5)]);
}

fn rgba8(c: [f32; 4]) -> [u8; 4] {
    c.map(|x| (x * 255.0).round() as u8)
}

/// A triangle in NVN's default lower-left origin: NDC +y is the picture's top, which is the last row in memory (the
/// picture is flipped at present); and the same triangle with an upper-left origin lands in the first rows.
#[test]
fn triangle() {
    let _g = setup();
    let e0 = validation_errors();
    for (origin, flip) in [(origin::LOWER_LEFT, false), (origin::UPPER_LEFT, true)] {
        let mut n = Nvn::with_origin(origin);
        let rt = n.texture(W, H, f::RGBA8, 0, false);
        let prog = tri_program(&mut n);
        let tri = [[-0.8f32, -0.6], [0.7, -0.5], [0.0, 0.9]];
        let color = [0.0, 1.0, 0.0, 1.0];
        n.begin();
        n.set_target(&rt, None, W, H);
        clear(&mut n, [0.0, 0.0, 1.0, 1.0]);
        bind_program(&mut n, &prog);
        draw_tri(&mut n, tri.map(|p| [p[0], p[1], 0.0]), color);
        n.submit_wait();
        let got = n.read_rgba8(&rt, W, H);
        let want: Vec<Option<[u8; 4]>> = tri_cover(tri, W, H, flip)
            .into_iter()
            .map(|c| c.map(|inside| if inside { rgba8(color) } else { [0, 0, 255, 255] }))
            .collect();
        check(if flip { "triangle upper-left" } else { "triangle lower-left" }, &got, &want, 0);
        // orientation: the apex (NDC y 0.9) is near the last row for lower-left, the first for upper-left
        let row = |y: u32| got[(y * W * 4 + 32 * 4) as usize..(y * W * 4 + 32 * 4 + 4) as usize].to_vec();
        let (top, bottom) = if flip { (row(6), row(57)) } else { (row(57), row(6)) };
        assert_eq!(top, vec![0, 255, 0, 255]);
        assert_eq!(bottom, vec![0, 0, 255, 255]);
    }
    assert_eq!(validation_errors(), e0, "validation errors");
}

/// A 4x4 checker texture filled by a buffer-to-texture copy, sampled nearest onto a quad drawn with 16-bit indices,
/// tinted by a uniform buffer.
#[test]
fn indexed_textured_quad() {
    let _g = setup();
    let e0 = validation_errors();
    let mut n = Nvn::new();
    let rt = n.texture(W, H, f::RGBA8, 0, false);
    let tex = n.texture(4, 4, f::RGBA8, 0, false);
    let checker: Vec<u8> = (0..16).flat_map(|i| if (i % 4 + i / 4) % 2 == 0 { [255u8, 255, 255, 255] } else { [255, 0, 0, 255] }).collect();
    let src = n.data.put(&checker, 256);
    n.register_texture(5, &tex);
    n.register_sampler(3, true);
    let prog = quad_program(&mut n);
    let verts: [f32; 16] = [-1.0, -1.0, 0.0, 0.0, 1.0, -1.0, 1.0, 0.0, 1.0, 1.0, 1.0, 1.0, -1.0, 1.0, 0.0, 1.0];
    let vb = n.data.put(&verts, 256);
    let idx: [u16; 6] = [0, 1, 2, 0, 2, 3];
    let ib = n.data.put(&idx, 256);
    let tint: [f32; 4] = [1.0, 1.0, 0.0, 1.0];
    let ub = n.data.put(&tint, 256);
    let handle = unsafe { nxgfx::nvn::device::nvnDeviceGetTextureHandle(&*n.dev, 5, 3) };
    n.begin();
    unsafe {
        let r = NVNcopyRegion { xoffset: 0, yoffset: 0, zoffset: 0, width: 4, height: 4, depth: 1 };
        nvnCommandBufferCopyBufferToTexture(n.c(), src, &*tex, core::ptr::null(), &r, 0);
    }
    n.set_target(&rt, None, W, H);
    clear(&mut n, [0.0, 0.0, 0.0, 1.0]);
    bind_program(&mut n, &prog);
    n.vertex_layout(&[(f::RG32F, 0), (f::RG32F, 8)], 16);
    unsafe {
        nvnCommandBufferBindVertexBuffer(n.c(), 0, vb, 64);
        nvnCommandBufferBindTexture(n.c(), stage::FRAGMENT as i32, 0, handle);
        nvnCommandBufferBindUniformBuffer(n.c(), stage::FRAGMENT as i32, 0, ub, 16);
        nvnCommandBufferDrawElements(n.c(), prim::TRIANGLES, index_type::U16, 6, ib);
    }
    n.submit_wait();
    let got = n.read_rgba8(&rt, W, H);
    // texel (u, v) = (x / 16, row / 16): uv (0,0) is row 0 of the texture, and NDC y -1 is row 0 of the target
    let want: Vec<Option<[u8; 4]>> = (0..W * H)
        .map(|i| {
            let (x, y) = (i % W, i / W);
            let (tx, ty) = (x / 16, y / 16);
            let white = (tx + ty) % 2 == 0;
            Some(if white { [255, 255, 0, 255] } else { [255, 0, 0, 255] })
        })
        .collect();
    check("indexed textured quad", &got, &want, 0);
    assert_eq!(validation_errors(), e0, "validation errors");
}

/// A triangle rendered into a texture, then that texture sampled onto a target twice its size.
#[test]
fn render_to_texture_then_sample() {
    let _g = setup();
    let e0 = validation_errors();
    let mut n = Nvn::new();
    let (aw, ah) = (32, 32);
    let a = n.texture(aw, ah, f::RGBA8, 0, false);
    let b = n.texture(W, H, f::RGBA8, 0, false);
    n.register_texture(7, &a);
    n.register_sampler(2, true);
    let tri = tri_program(&mut n);
    let quad = quad_program(&mut n);
    let verts: [f32; 16] = [-1.0, -1.0, 0.0, 0.0, 1.0, -1.0, 1.0, 0.0, 1.0, 1.0, 1.0, 1.0, -1.0, 1.0, 0.0, 1.0];
    let vb = n.data.put(&verts, 256);
    let ub = n.data.put(&[1.0f32, 1.0, 1.0, 1.0], 256);
    let handle = unsafe { nxgfx::nvn::device::nvnDeviceGetTextureHandle(&*n.dev, 7, 2) };
    let t = [[-0.9f32, -0.9], [0.9, -0.9], [-0.9, 0.9]];
    n.begin();
    n.set_target(&a, None, aw, ah);
    clear(&mut n, [0.0, 0.0, 0.0, 1.0]);
    bind_program(&mut n, &tri);
    draw_tri(&mut n, t.map(|p| [p[0], p[1], 0.0]), [1.0, 0.5, 0.0, 1.0]);
    unsafe { nvnCommandBufferBarrier(n.c(), (barrier::ORDER_FRAGMENTS | barrier::INVALIDATE_TEXTURE) as i32) };
    n.set_target(&b, None, W, H);
    clear(&mut n, [0.0, 0.0, 1.0, 1.0]);
    bind_program(&mut n, &quad);
    n.vertex_layout(&[(f::RG32F, 0), (f::RG32F, 8)], 16);
    unsafe {
        nvnCommandBufferBindVertexBuffer(n.c(), 0, vb, 64);
        nvnCommandBufferBindTexture(n.c(), stage::FRAGMENT as i32, 0, handle);
        nvnCommandBufferBindUniformBuffer(n.c(), stage::FRAGMENT as i32, 0, ub, 16);
        nvnCommandBufferDrawArrays(n.c(), prim::TRIANGLE_FAN, 0, 4);
    }
    n.submit_wait();
    let got_a = n.read_rgba8(&a, aw, ah);
    let want_a: Vec<Option<[u8; 4]>> =
        tri_cover(t, aw, ah, false).into_iter().map(|c| c.map(|i| if i { [255, 128, 0, 255] } else { [0, 0, 0, 255] })).collect();
    check("render target A", &got_a, &want_a, 1);
    let got_b = n.read_rgba8(&b, W, H);
    let want_b: Vec<Option<[u8; 4]>> = (0..W * H)
        .map(|i| {
            let (x, y) = (i % W, i / W);
            let k = ((y / 2) * aw + x / 2) as usize;
            Some([got_a[4 * k], got_a[4 * k + 1], got_a[4 * k + 2], got_a[4 * k + 3]])
        })
        .collect();
    check("sampled into B", &got_b, &want_b, 0);
    assert_eq!(validation_errors(), e0, "validation errors");
}

/// Depth test (LESS, NVN's default near = -w depth mode) and alpha blending.
#[test]
fn blending_and_depth() {
    let _g = setup();
    let e0 = validation_errors();
    let mut n = Nvn::new();
    let rt = n.texture(W, H, f::RGBA8, 0, false);
    let depth = n.texture(W, H, nxgfx::nvn::types::fmt::DEPTH24_STENCIL8, 0, false);
    let prog = tri_program(&mut n);
    // NDC rectangles as two triangles each
    let rect = |n: &mut Nvn, x0: f32, x1: f32, y0: f32, y1: f32, z: f32, c: [f32; 4]| {
        draw_tri(n, [[x0, y0, z], [x1, y0, z], [x1, y1, z]], c);
        draw_tri(n, [[x0, y0, z], [x1, y1, z], [x0, y1, z]], c);
    };
    n.begin();
    n.set_target(&rt, Some(&depth), W, H);
    clear(&mut n, [0.0, 0.0, 0.0, 1.0]);
    unsafe { nvnCommandBufferClearDepthStencil(n.c(), 1.0, 1, 0, 0xff) };
    bind_program(&mut n, &prog);
    let mut ds = NVNdepthStencilState::boxed();
    unsafe {
        nvnDepthStencilStateSetDefaults(&mut *ds);
        nvnDepthStencilStateSetDepthTestEnable(&mut *ds, 1);
        nvnDepthStencilStateSetDepthWriteEnable(&mut *ds, 1);
        nvnDepthStencilStateSetDepthFunc(&mut *ds, cmp::LESS as i32);
        nvnCommandBufferBindDepthStencilState(n.c(), &*ds);
    }
    // A: red, depth 0.5 (NDC z 0), x in [-1, 0.25) -> columns 0..40
    rect(&mut n, -1.0, 0.25, -1.0, 1.0, 0.0, [1.0, 0.0, 0.0, 1.0]);
    // B: green, depth 0.7 (NDC z 0.4), columns 20..64: behind A where they overlap
    rect(&mut n, -0.375, 1.0, -1.0, 1.0, 0.4, [0.0, 1.0, 0.0, 1.0]);
    // C: blue at half alpha, depth 0.1 (NDC z -0.8), rows 0..32, blended over, no depth write
    let mut cs = NVNcolorState::boxed();
    let mut bs = NVNblendState::boxed();
    unsafe {
        nvnColorStateSetDefaults(&mut *cs);
        nvnColorStateSetBlendEnable(&mut *cs, 0, 1);
        nvnCommandBufferBindColorState(n.c(), &*cs);
        nvnBlendStateSetDefaults(&mut *bs);
        nvnBlendStateSetBlendFunc(&mut *bs, blend_func::SRC_ALPHA as i32, blend_func::ONE_MINUS_SRC_ALPHA as i32, blend_func::ONE as i32, blend_func::ZERO as i32);
        nvnCommandBufferBindBlendState(n.c(), &*bs);
        nvnDepthStencilStateSetDepthWriteEnable(&mut *ds, 0);
        nvnCommandBufferBindDepthStencilState(n.c(), &*ds);
    }
    rect(&mut n, -1.0, 1.0, -1.0, 0.0, -0.8, [0.0, 0.0, 1.0, 0.5]);
    n.submit_wait();
    let got = n.read_rgba8(&rt, W, H);
    let want: Vec<Option<[u8; 4]>> = (0..W * H)
        .map(|i| {
            let (x, y) = (i % W, i / W);
            if x == 20 || x == 40 || y == 32 {
                return None;
            }
            let base: [f32; 4] = if x < 40 { [1.0, 0.0, 0.0, 1.0] } else { [0.0, 1.0, 0.0, 1.0] };
            // alpha: src alpha x ONE + dst alpha x ZERO = 0.5
            let c = if y < 32 { [base[0] * 0.5, base[1] * 0.5, 0.5, 0.5] } else { base };
            Some(c.map(|v| (v * 255.0).round() as u8))
        })
        .collect();
    check("blending and depth", &got, &want, 1);
    assert_eq!(validation_errors(), e0, "validation errors");
}

/// A compute dispatch writes a storage buffer in a CPU-visible pool; after the fence the CPU reads what it wrote
/// (the mirror's write-back), and a buffer copy of it lands too.
#[test]
fn compute_writes_buffer_visible_after_fence() {
    let _g = setup();
    let e0 = validation_errors();
    for placement in ["mirror", "host"] {
        std::env::set_var("NXGFX_POOL", placement);
        let mut n = Nvn::new();
        let mut out = Pool::new(n.d(), 1 << 16, CPU_POOL);
        std::env::remove_var("NXGFX_POOL");
        let cs = spv("fill_comp");
        let cj = json(
            "cs",
            r#"{"set": 0, "binding": 0, "type": "uniform_buffer", "count": 1, "name": "support_buffer", "resource": {"kind": "support"}},
               {"set": 5, "binding": 1, "type": "storage_buffer", "count": 1, "name": "s0", "resource": {"kind": "sbuf", "slot": 0, "cb_slot": 0, "cb_offset": 196, "flags": 1}},
               {"set": 5, "binding": 2, "type": "uniform_buffer", "count": 1, "name": "c3", "resource": {"kind": "cbuf", "slot": 3, "fixed": 0}}"#,
        );
        // the shader's json declares no support buffer use in its SPIR-V: the remap only needs entries it has
        let prog = n.program(&[(&cs, &cj)]);
        let (soff, saddr) = out.alloc(256 * 4, 256);
        let (coff, caddr) = out.alloc(256 * 4, 256);
        out.mem.slice()[soff..soff + 1024].fill(0xee);
        out.mem.slice()[coff..coff + 1024].fill(0xee);
        let ub = n.data.put(&[1000u32, 0, 0, 0], 256);
        n.begin();
        unsafe {
            nvnCommandBufferBindProgram(n.c(), &*prog, 0x20);
            nvnCommandBufferBindStorageBuffer(n.c(), stage::COMPUTE as i32, 0, saddr, 1024);
            nvnCommandBufferBindUniformBuffer(n.c(), stage::COMPUTE as i32, 0, ub, 16);
            nvnCommandBufferDispatchCompute(n.c(), 4, 1, 1);
            nvnCommandBufferBarrier(n.c(), barrier::ORDER_PRIMITIVES as i32);
            nvnCommandBufferCopyBufferToBuffer(n.c(), saddr, caddr, 1024, 0);
        }
        n.submit_wait();
        let s = &out.mem.slice()[soff..soff + 1024];
        let c = &out.mem.slice()[coff..coff + 1024];
        let mut bad = 0;
        for i in 0..256 {
            let v = u32::from_le_bytes(s[4 * i..4 * i + 4].try_into().unwrap());
            let w = u32::from_le_bytes(c[4 * i..4 * i + 4].try_into().unwrap());
            if v != i as u32 * 3 + 1000 || w != v {
                bad += 1;
            }
        }
        eprintln!("compute ({placement}): 256 words written, {bad} wrong");
        assert_eq!(bad, 0, "compute result in guest memory ({placement})");
    }
    assert_eq!(validation_errors(), e0, "validation errors");
}

/// GPU writes reach guest memory at the fence and CPU writes made after reach the GPU at the next submission:
/// a texture copied to a CPU-visible buffer, then the CPU rewrites that buffer and the GPU copies it on.
#[test]
fn pool_write_back_round_trip() {
    let _g = setup();
    let e0 = validation_errors();
    let mut n = Nvn::new();
    let rt = n.texture(16, 16, f::RGBA8, 0, false);
    let (aoff, a) = n.data.alloc(16 * 16 * 4, 256);
    let (boff, b) = n.data.alloc(16 * 16 * 4, 256);
    n.begin();
    n.set_target(&rt, None, 16, 16);
    clear(&mut n, [0.25, 0.5, 0.75, 1.0]);
    unsafe {
        let r = NVNcopyRegion { xoffset: 0, yoffset: 0, zoffset: 0, width: 16, height: 16, depth: 1 };
        nvnCommandBufferCopyTextureToBuffer(n.c(), &*rt, core::ptr::null(), &r, a, 0);
    }
    n.submit_wait();
    let got: Vec<u8> = n.data.mem.slice()[aoff..aoff + 1024].to_vec();
    // 0.25 / 0.5 / 0.75 in UNORM8 (a tie rounds either way)
    let near = |p: &[u8]| p[0].abs_diff(64) <= 1 && p[1].abs_diff(128) <= 1 && p[2].abs_diff(191) <= 1 && p[3] == 255;
    assert!(got.chunks(4).all(near), "texture copy in guest memory: {:?}", &got[..8]);
    // the CPU writes the buffer after the GPU did; the next submission copies it on
    for (i, x) in n.data.mem.slice()[aoff..aoff + 1024].iter_mut().enumerate() {
        *x = (i * 7) as u8;
    }
    n.begin();
    unsafe { nvnCommandBufferCopyBufferToBuffer(n.c(), a, b, 1024, 0) };
    n.submit_wait();
    let s = n.data.mem.slice();
    let ok = (0..1024).all(|i| s[boff + i] == (i * 7) as u8);
    eprintln!("write-back round trip: GPU->CPU 1024 bytes, CPU->GPU->CPU 1024 bytes: {}", if ok { "equal" } else { "differ" });
    assert!(ok, "CPU writes after a GPU write reach the GPU");
    assert_eq!(validation_errors(), e0, "validation errors");
}

/// An event wait no value satisfies parks its queue: queue A waits for an event and then copies; it is submitted
/// before queue B, which signals the event. A's work runs only once B's signal lands, and A's fence (a receipt of
/// the parked queue) is reached only then.
#[test]
fn event_wait_parks_a_queue_until_another_queue_signals() {
    use nxgfx::nvn::queue::*;
    use nxgfx::nvn::sync::*;
    let _g = setup();
    let e0 = validation_errors();
    let mut n = Nvn::new();
    let d = n.d();
    // queue B and a command buffer of its own
    let mut qb = NVNqueueBuilder::boxed();
    let mut qb_obj = NVNqueue::boxed();
    let mut cmd_b = NVNcommandBuffer::boxed();
    let mut ctl_b = vec![0u64; 512];
    let mut cmd_pool_b = Pool::new(d, 1 << 16, CPU_POOL);
    unsafe {
        nvnQueueBuilderSetDevice(&mut *qb, d);
        nvnQueueBuilderSetDefaults(&mut *qb);
        assert_eq!(nvnQueueInitialize(&mut *qb_obj, &*qb), 1);
        nvnCommandBufferInitialize(&mut *cmd_b, d);
        nvnCommandBufferAddCommandMemory(&mut *cmd_b, cmd_pool_b.p(), 0, 1 << 16);
        nvnCommandBufferAddControlMemory(&mut *cmd_b, ctl_b.as_mut_ptr() as *mut u8, ctl_b.len() * 8);
    }
    let (eoff, _) = n.data.alloc(16, 16);
    n.data.mem.slice()[eoff..eoff + 4].fill(0);
    let mut eb = NVNeventBuilder::boxed();
    let mut ev = NVNevent::boxed();
    unsafe {
        nvnEventBuilderSetDefaults(&mut *eb);
        nvnEventBuilderSetStorage(&mut *eb, n.data.p(), eoff as i64);
        assert_eq!(nvnEventInitialize(&mut *ev, &*eb), 1);
    }
    let pattern: Vec<u8> = (0..256).map(|i| (i * 13 + 5) as u8).collect();
    let src = n.data.put(&pattern, 256);
    let (doff, dst) = n.data.alloc(256, 256);
    n.data.mem.slice()[doff..doff + 256].fill(0);
    let mut sync_a = NVNsync::boxed();
    let mut sync_b = NVNsync::boxed();
    unsafe {
        nvnSyncInitialize(&mut *sync_a, d);
        nvnSyncInitialize(&mut *sync_b, d);
        nvnCommandBufferBeginRecording(n.c());
        nvnCommandBufferWaitEvent(n.c(), &*ev, event_wait::EQUAL as i32, 1);
        nvnCommandBufferCopyBufferToBuffer(n.c(), src, dst, 256, 0);
        let h = nvnCommandBufferEndRecording(n.c());
        nvnQueueSubmitCommands(&mut *n.queue, 1, &h);
        nvnQueueFenceSync(&mut *n.queue, &mut *sync_a, 0, 0);
    }
    std::thread::sleep(std::time::Duration::from_millis(100));
    let early = unsafe { nvnSyncWait(&*sync_a, 0) };
    assert_eq!(early, sync_wait::TIMEOUT_EXPIRED, "queue A ran before the event");
    assert!(n.data.mem.slice()[doff..doff + 256].iter().all(|&b| b == 0), "the copy behind the wait ran early");
    unsafe {
        nvnCommandBufferBeginRecording(&mut *cmd_b);
        nvnCommandBufferSignalEvent(&mut *cmd_b, &*ev, event_signal::WRITE as i32, 0, 0, 1);
        let h = nvnCommandBufferEndRecording(&mut *cmd_b);
        nvnQueueSubmitCommands(&mut *qb_obj, 1, &h);
        nvnQueueFenceSync(&mut *qb_obj, &mut *sync_b, 0, 0);
        let r = nvnSyncWait(&*sync_b, 5_000_000_000);
        assert!(r != sync_wait::TIMEOUT_EXPIRED);
        let r = nvnSyncWait(&*sync_a, 5_000_000_000);
        assert!(r == sync_wait::CONDITION_SATISFIED || r == sync_wait::ALREADY_SIGNALED, "queue A ran on after the signal: {r}");
        assert_eq!(nvnEventGetValue(&*ev), 1);
    }
    assert_eq!(&n.data.mem.slice()[doff..doff + 256], &pattern[..], "the parked copy ran after the signal");
    eprintln!("queue parking: A held by its event wait until B signalled, then ran; its receipt fence resolved");
    unsafe {
        nvnCommandBufferFinalize(&mut *cmd_b);
        nvnQueueFinalize(&mut *qb_obj);
    }
    assert_eq!(validation_errors(), e0, "validation errors");
}

/// WriteTexels lands in the texture's storage: another texture object over the same storage reads it (Mario Kart 8
/// Deluxe's menu preview: the movie decoder writes the client's texture, the menu samples its own), also after its
/// image exists (a later frame), and a partial region keeps the rest.
#[test]
fn write_texels_reach_every_texture_over_the_storage() {
    let _g = setup();
    let e0 = validation_errors();
    let mut n = Nvn::new();
    let (w, h) = (40u32, 24u32);
    let a = n.texture(w, h, f::RGBA8, 0, true);
    // a second object over the same storage (as a game initializes its own over the decoder's buffer)
    let b = unsafe {
        let mut tb = NVNtextureBuilder::boxed();
        let mut t = NVNtexture::boxed();
        nvnTextureBuilderSetDevice(&mut *tb, n.d());
        nvnTextureBuilderSetDefaults(&mut *tb);
        nvnTextureBuilderSetSize2D(&mut *tb, w as i32, h as i32);
        nvnTextureBuilderSetFormat(&mut *tb, f::RGBA8);
        nvnTextureBuilderSetStorage(&mut *tb, nvnTextureGetMemoryPool(&*a), nvnTextureGetMemoryOffset(&*a));
        assert_eq!(nvnTextureInitialize(&mut *t, &*tb), 1);
        t
    };
    let frame = |k: u8| -> Vec<u8> { (0..w * h).flat_map(|i| [(i % 251) as u8, k, (i / w) as u8, 255]).collect() };
    let whole = NVNcopyRegion { xoffset: 0, yoffset: 0, zoffset: 0, width: w as i32, height: h as i32, depth: 1 };
    let f1 = frame(10);
    unsafe { nvnTextureWriteTexels(&*a, core::ptr::null(), &whole, f1.as_ptr()) };
    assert!(n.read_rgba8(&b, w, h) == f1, "first frame through the other object");
    let f2 = frame(20);
    unsafe { nvnTextureWriteTexelsStrided(&*a, core::ptr::null(), &whole, f2.as_ptr(), (w * 4) as isize, 0) };
    assert!(n.read_rgba8(&b, w, h) == f2, "second frame: the existing image uploads again");
    // a 5x3 region at (7, 9), rows of 5 texels
    let part: Vec<u8> = (0..15).flat_map(|_| [1u8, 2, 3, 4]).collect();
    let r = NVNcopyRegion { xoffset: 7, yoffset: 9, zoffset: 0, width: 5, height: 3, depth: 1 };
    unsafe { nvnTextureWriteTexels(&*a, core::ptr::null(), &r, part.as_ptr()) };
    let got = n.read_rgba8(&b, w, h);
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let inside = (7..12).contains(&x) && (9..12).contains(&y);
            let want = if inside { [1u8, 2, 3, 4] } else { [f2[i], f2[i + 1], f2[i + 2], f2[i + 3]] };
            assert_eq!(&got[i..i + 4], &want, "texel {x},{y}");
        }
    }
    assert_eq!(validation_errors(), e0, "validation errors");
}

/// Resolution scale 2: a compressible target is drawn at twice its size (viewport scaled) and read back at the
/// game's size through the bounce image; the picture matches the unscaled one away from the triangle's edges.
#[test]
fn resolution_scale_renders_at_twice_the_size() {
    let _g = setup();
    let e0 = validation_errors();
    nxconfig::set("resolution", "2");
    let mut n = Nvn::new();
    let rt = n.texture(W, H, f::RGBA8, nxgfx::nvn::types::tex_flags::COMPRESSIBLE, false);
    let prog = tri_program(&mut n);
    let tri = [[-0.8f32, -0.6], [0.7, -0.5], [0.0, 0.9]];
    let color = [0.0, 1.0, 0.0, 1.0];
    n.begin();
    n.set_target(&rt, None, W, H);
    clear(&mut n, [0.0, 0.0, 1.0, 1.0]);
    bind_program(&mut n, &prog);
    draw_tri(&mut n, tri.map(|p| [p[0], p[1], 0.0]), color);
    n.submit_wait();
    assert!(nxgfx::vk::scale::active(), "the target was made scaled");
    let got = n.read_rgba8(&rt, W, H);
    let want: Vec<Option<[u8; 4]>> = tri_cover(tri, W, H, false)
        .into_iter()
        .map(|c| c.map(|inside| if inside { rgba8(color) } else { [0, 0, 255, 255] }))
        .collect();
    nxconfig::set("resolution", "1");
    check("triangle at resolution x2", &got, &want, 0);
    assert_eq!(validation_errors(), e0, "validation errors");
}

/// A texture made without NVN's IMAGE flag, registered as an image and written by image stores (Turbo caa07ad,
/// `NXGFX_IMAGE_NOFLAG=1`): its image gets storage use (an sRGB one through its UNORM twin) and the writes land. The
/// texture is registered as a sampled texture first, so its image is made without storage use and made again.
#[test]
fn image_stores_to_a_texture_without_the_image_flag() {
    let _g = setup();
    let e0 = validation_errors();
    std::env::set_var("NXGFX_IMAGE_NOFLAG", "1");
    for (name, fmt) in [("RGBA8", f::RGBA8), ("RGBA8_SRGB", 56)] {
        let mut n = Nvn::new();
        let tex = n.texture(W, H, fmt, 0, false);
        n.register_texture(9, &tex);
        // the image exists without storage use before the image registration
        n.begin();
        let r = NVNcopyRegion { xoffset: 0, yoffset: 0, zoffset: 0, width: W as i32, height: H as i32, depth: 1 };
        unsafe { nvnCommandBufferClearTexture(n.c(), &*tex, core::ptr::null(), &r, [0.0f32; 4].as_ptr(), 0xf) };
        n.submit_wait();
        unsafe { nxgfx::nvn::pool::nvnTexturePoolRegisterImage(&*n.tex_pool, 10, &*tex, core::ptr::null()) };
        let cs = spv("image_store_comp");
        let cj = json(
            "cs",
            r#"{"set": 5, "binding": 1, "type": "storage_image", "count": 1, "name": "img", "resource": {"kind": "image", "handle": 72, "cbuf_slot": -1, "type": 3, "format": "Unknown"}}"#,
        );
        let prog = n.program(&[(&cs, &cj)]);
        let h = unsafe { nxgfx::nvn::device::nvnDeviceGetImageHandle(&*n.dev, 10) };
        n.begin();
        unsafe {
            nvnCommandBufferBindProgram(n.c(), &*prog, 0x20);
            nvnCommandBufferBindImage(n.c(), stage::COMPUTE as i32, 0, h);
            nvnCommandBufferDispatchCompute(n.c(), W as i32 / 8, H as i32 / 8, 1);
            nvnCommandBufferBarrier(n.c(), barrier::ORDER_PRIMITIVES as i32);
        }
        n.submit_wait();
        let got = n.read_rgba8(&tex, W, H);
        let want: Vec<Option<[u8; 4]>> = (0..W * H).map(|i| Some([(i % W) as u8, (i / W) as u8, 128, 255])).collect();
        check(&format!("image stores into {name} without the IMAGE flag"), &got, &want, 1);
    }
    std::env::remove_var("NXGFX_IMAGE_NOFLAG");
    assert_eq!(validation_errors(), e0, "validation errors");
}

/// A pixel shader whose float output writes an R32UI target (the translator's outputs are float register containers):
/// drawn through its integer-output variant (Turbo's `vkspv_int_outputs`), the target gets the bits it wrote.
#[test]
fn float_output_bits_reach_an_integer_target() {
    let _g = setup();
    let e0 = validation_errors();
    let mut n = Nvn::new();
    let rt = n.texture(W, H, 11, 0, false); // R32UI
    let vs = spv("tri_vert");
    let fs = spv("int_out_frag");
    let prog = n.program(&[(&vs, &json("vs", "")), (&fs, &json("ps", ""))]);
    n.begin();
    n.set_target(&rt, None, W, H);
    bind_program(&mut n, &prog);
    draw_tri(&mut n, [[-1.0, -1.0, 0.0], [3.0, -1.0, 0.0], [-1.0, 3.0, 0.0]], [0.0; 4]);
    n.submit_wait();
    let got = n.read_rgba8(&rt, W, H);
    let want: Vec<Option<[u8; 4]>> = (0..W * H).map(|_| Some(0x1234_5678u32.to_le_bytes())).collect();
    check("float output bits into R32UI", &got, &want, 0);
    assert_eq!(validation_errors(), e0, "validation errors");
}

/// NVN's alpha test, applied in the pixel shader (Turbo `vkspv_alpha_test`): with GREATER 0.5 the half drawn at
/// alpha 0.3 is discarded and the half at alpha 0.7 is drawn.
#[test]
fn alpha_test_discards_in_the_shader() {
    let _g = setup();
    let e0 = validation_errors();
    let mut n = Nvn::new();
    let rt = n.texture(W, H, f::RGBA8, 0, false);
    let prog = tri_program(&mut n);
    n.begin();
    n.set_target(&rt, None, W, H);
    clear(&mut n, [0.0, 0.0, 1.0, 1.0]);
    bind_program(&mut n, &prog);
    let mut cs = NVNcolorState::boxed();
    unsafe {
        nvnColorStateSetDefaults(&mut *cs);
        nvnColorStateSetAlphaTest(&mut *cs, 5); // GREATER
        nvnCommandBufferBindColorState(n.c(), &*cs);
        nxgfx::nvn::cmd2::nvnCommandBufferSetAlphaRef(n.c(), 0.5);
    }
    for (x0, x1, a) in [(-1.0f32, 0.0f32, 0.3f32), (0.0, 1.0, 0.7)] {
        let c = [1.0, 0.0, 0.0, a];
        draw_tri(&mut n, [[x0, -1.0, 0.0], [x1, -1.0, 0.0], [x1, 1.0, 0.0]], c);
        draw_tri(&mut n, [[x0, -1.0, 0.0], [x1, 1.0, 0.0], [x0, 1.0, 0.0]], c);
    }
    n.submit_wait();
    let got = n.read_rgba8(&rt, W, H);
    let want: Vec<Option<[u8; 4]>> = (0..W * H)
        .map(|i| {
            let x = i % W;
            if x == 32 {
                None
            } else if x < 32 {
                Some([0, 0, 255, 255])
            } else {
                Some([255, 0, 0, 179])
            }
        })
        .collect();
    check("alpha test GREATER 0.5", &got, &want, 1);
    assert_eq!(validation_errors(), e0, "validation errors");
}

/// Dual-source blending: the pixel shader's output 1 is target 0's second source (Turbo `vkspv_dual_source`):
/// colour = src x ONE + dst x SRC1_COLOR over a white target.
#[test]
fn dual_source_blending_reads_output_1() {
    let _g = setup();
    let e0 = validation_errors();
    let mut n = Nvn::new();
    let rt = n.texture(W, H, f::RGBA8, 0, false);
    let vs = spv("tri_vert");
    let fs = spv("dual_frag");
    let prog = n.program(&[(&vs, &json("vs", "")), (&fs, &json("ps", ""))]);
    n.begin();
    n.set_target(&rt, None, W, H);
    clear(&mut n, [1.0, 1.0, 1.0, 1.0]);
    bind_program(&mut n, &prog);
    let mut cs = NVNcolorState::boxed();
    let mut bs = NVNblendState::boxed();
    unsafe {
        nvnColorStateSetDefaults(&mut *cs);
        nvnColorStateSetBlendEnable(&mut *cs, 0, 1);
        nvnCommandBufferBindColorState(n.c(), &*cs);
        nvnBlendStateSetDefaults(&mut *bs);
        nvnBlendStateSetBlendFunc(&mut *bs, blend_func::ONE as i32, 16 /* SRC1_COLOR */, blend_func::ONE as i32, blend_func::ZERO as i32);
        nvnCommandBufferBindBlendState(n.c(), &*bs);
    }
    draw_tri(&mut n, [[-1.0, -1.0, 0.0], [3.0, -1.0, 0.0], [-1.0, 3.0, 0.0]], [0.25, 0.0, 0.0, 1.0]);
    n.submit_wait();
    let got = n.read_rgba8(&rt, W, H);
    let want: Vec<Option<[u8; 4]>> = (0..W * H).map(|_| Some([128, 128, 255, 255])).collect();
    check("dual-source blending", &got, &want, 1);
    assert_eq!(validation_errors(), e0, "validation errors");
}

/// A buffer texture (texel buffer view over its pool memory) read by texelFetch in a compute shader.
#[test]
fn buffer_texture_read_by_texel_fetch() {
    let _g = setup();
    let e0 = validation_errors();
    let mut n = Nvn::new();
    let src: Vec<u32> = (0..256u32).map(|i| i * 7 + 3).collect();
    let (soff, _) = n.data.alloc(1024, 256);
    unsafe { core::ptr::copy_nonoverlapping(src.as_ptr() as *const u8, n.data.mem.ptr.add(soff), 1024) };
    let mut b = NVNtextureBuilder::boxed();
    let mut tex = NVNtexture::boxed();
    unsafe {
        nvnTextureBuilderSetDevice(&mut *b, n.d());
        nvnTextureBuilderSetDefaults(&mut *b);
        nvnTextureBuilderSetTarget(&mut *b, nxgfx::nvn::types::target::BUFFER);
        nvnTextureBuilderSetFormat(&mut *b, 11); // R32UI
        nvnTextureBuilderSetSize1D(&mut *b, 256);
        nvnTextureBuilderSetStorage(&mut *b, n.data.p(), soff as isize);
        assert_eq!(nvnTextureInitialize(&mut *tex, &*b), 1);
    }
    n.register_texture(6, &tex);
    let mut out = Pool::new(n.d(), 1 << 16, CPU_POOL);
    let (ooff, oaddr) = out.alloc(1024, 256);
    let cs = spv("tbuf_comp");
    let cj = json(
        "cs",
        r#"{"set": 5, "binding": 1, "type": "uniform_texel_buffer", "count": 1, "name": "tb", "resource": {"kind": "texture", "handle": 8, "cbuf_slot": -1, "type": 2}},
           {"set": 5, "binding": 2, "type": "storage_buffer", "count": 1, "name": "o", "resource": {"kind": "sbuf", "slot": 0, "cb_slot": 0, "cb_offset": 196, "flags": 1}}"#,
    );
    let prog = n.program(&[(&cs, &cj)]);
    let h = unsafe { nxgfx::nvn::device::nvnDeviceGetTextureHandle(&*n.dev, 6, 0) };
    n.begin();
    unsafe {
        nvnCommandBufferBindProgram(n.c(), &*prog, 0x20);
        nvnCommandBufferBindTexture(n.c(), stage::COMPUTE as i32, 0, h);
        nvnCommandBufferBindStorageBuffer(n.c(), stage::COMPUTE as i32, 0, oaddr, 1024);
        nvnCommandBufferDispatchCompute(n.c(), 4, 1, 1);
        nvnCommandBufferBarrier(n.c(), barrier::ORDER_PRIMITIVES as i32);
    }
    n.submit_wait();
    let o = &out.mem.slice()[ooff..ooff + 1024];
    let bad = (0..256).filter(|&i| u32::from_le_bytes(o[4 * i..4 * i + 4].try_into().unwrap()) != src[i] * 2 + 1).count();
    eprintln!("buffer texture: 256 texels fetched, {bad} wrong");
    assert_eq!(bad, 0, "texelFetch through the buffer texture");
    assert_eq!(validation_errors(), e0, "validation errors");
}

/// An append counter as indirect-draw arguments (Donkey Kong Bananza's terrain, `exec.rs` mod append): a draw's
/// vertex shader adds 6 to word 0 of a 64-byte record it binds as a written storage buffer; DrawElementsIndirect then
/// draws with that record (its instance count still 0): it gets one instance and draws the 6 indices of a full quad.
#[test]
fn append_counter_as_indirect_draw_arguments() {
    let _g = setup();
    let e0 = validation_errors();
    let mut n = Nvn::new();
    let rt = n.texture(W, H, f::RGBA8, 0, false);
    let vs = spv("append_vert");
    let vj = json("vs", r#"{"set": 0, "binding": 1, "type": "storage_buffer", "count": 1, "name": "rec", "resource": {"kind": "sbuf", "slot": 0, "cb_slot": 0, "cb_offset": 68, "flags": 1}}"#);
    let fs = spv("tri_frag");
    let counting = n.program(&[(&vs, &vj), (&fs, &json("ps", ""))]);
    let tri = tri_program(&mut n);
    let rec = n.data.put(&[0u32; 16], 256);
    let quad: Vec<f32> = [[-1.0f32, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]].iter().flat_map(|p| [p[0], p[1], 0.0, 0.0, 1.0, 0.0, 1.0]).collect();
    let qb = n.data.put(&quad, 256);
    let ib = n.data.put(&[0u16, 1, 2, 0, 2, 3], 256);
    n.begin();
    n.set_target(&rt, None, W, H);
    clear(&mut n, [0.0, 0.0, 1.0, 1.0]);
    bind_program(&mut n, &counting);
    unsafe { nvnCommandBufferBindStorageBuffer(n.c(), stage::VERTEX as i32, 0, rec, 64) };
    // a degenerate triangle: only its vertex shader's count matters
    draw_tri(&mut n, [[0.0, 0.0, 0.0]; 3], [0.0; 4]);
    unsafe { nvnCommandBufferBarrier(n.c(), (barrier::ORDER_PRIMITIVES | barrier::ORDER_INDIRECT_DATA) as i32) };
    bind_program(&mut n, &tri);
    n.vertex_layout(&[(f::RGB32F, 0), (f::RGBA32F, 12)], 28);
    unsafe {
        nvnCommandBufferBindVertexBuffer(n.c(), 0, qb, quad.len() * 4);
        nvnCommandBufferDrawElementsIndirect(n.c(), prim::TRIANGLES, index_type::U16, ib, rec);
    }
    n.submit_wait();
    let got = n.read_rgba8(&rt, W, H);
    let want: Vec<Option<[u8; 4]>> = (0..W * H).map(|_| Some([0, 255, 0, 255])).collect();
    check("indirect draw from an append counter", &got, &want, 0);
    assert_eq!(validation_errors(), e0, "validation errors");
}

/// A linear render target in a CPU-visible pool (Turbo's `rt_readback`): what a pass draws there is in its storage
/// for the CPU at the fence, without a texel read or a copy by the game.
#[test]
fn linear_render_target_reaches_guest_memory() {
    let _g = setup();
    let e0 = validation_errors();
    let mut n = Nvn::new();
    let stride = W as usize * 4 + 256;
    let mut b = NVNtextureBuilder::boxed();
    let mut rt = NVNtexture::boxed();
    let off;
    unsafe {
        nvnTextureBuilderSetDevice(&mut *b, n.d());
        nvnTextureBuilderSetDefaults(&mut *b);
        nvnTextureBuilderSetSize2D(&mut *b, W as i32, H as i32);
        nvnTextureBuilderSetFormat(&mut *b, f::RGBA8);
        nvnTextureBuilderSetFlags(&mut *b, nxgfx::nvn::types::tex_flags::LINEAR_RENDER_TARGET as i32);
        nvnTextureBuilderSetStride(&mut *b, stride as isize);
        let size = nvnTextureBuilderGetStorageSize(&*b).max(stride * H as usize);
        let (o, _) = n.data.alloc(size, 4096);
        off = o;
        n.data.mem.slice()[o..o + size].fill(0x11);
        nvnTextureBuilderSetStorage(&mut *b, n.data.p(), o as isize);
        assert_eq!(nvnTextureInitialize(&mut *rt, &*b), 1);
    }
    let prog = tri_program(&mut n);
    n.begin();
    n.set_target(&rt, None, W, H);
    clear(&mut n, [0.0, 0.0, 1.0, 1.0]);
    bind_program(&mut n, &prog);
    draw_tri(&mut n, [[-1.0, -1.0, 0.0], [3.0, -1.0, 0.0], [-1.0, 3.0, 0.0]], [1.0, 0.5, 0.0, 1.0]);
    n.submit_wait();
    let m = &n.data.mem.slice()[off..off + stride * H as usize];
    let mut bad = 0;
    for y in 0..H as usize {
        for x in 0..W as usize {
            let t = &m[y * stride + 4 * x..y * stride + 4 * x + 4];
            if t[0] != 255 || !(127..=128).contains(&t[1]) || t[2] != 0 || t[3] != 255 {
                bad += 1;
            }
        }
    }
    eprintln!("linear render target in guest memory: {} texels, {bad} wrong (first {:?})", W * H, &m[..8]);
    assert_eq!(bad, 0, "the drawn texels in the storage");
    assert_eq!(validation_errors(), e0, "validation errors");
}

/// NVN's warp-aggregated atomic counter after an early return (hangs RTX 50 cards, Turbo #12): the module is loaded
/// with each ballot made per lane (`shaders::link::ballot_per_lane`) and the count is still right.
#[test]
fn ballot_counter_made_per_lane_counts_right() {
    let _g = setup();
    let e0 = validation_errors();
    let mut n = Nvn::new();
    let mut out = Pool::new(n.d(), 1 << 16, CPU_POOL);
    let (coff, caddr) = out.alloc(256, 256);
    out.mem.slice()[coff..coff + 4].copy_from_slice(&0u32.to_le_bytes());
    let data: Vec<u32> = (0..256u32).map(|i| (i % 3 != 0) as u32).collect();
    let want = data.iter().filter(|&&d| d != 0).count() as u32;
    let daddr = n.data.put(&data, 256);
    let cs = spv("ballot_comp");
    let cj = json(
        "cs",
        r#"{"set": 5, "binding": 1, "type": "storage_buffer", "count": 1, "name": "count", "resource": {"kind": "sbuf", "slot": 0, "cb_slot": 0, "cb_offset": 196, "flags": 1}},
           {"set": 5, "binding": 2, "type": "storage_buffer", "count": 1, "name": "data", "resource": {"kind": "sbuf", "slot": 1, "cb_slot": 0, "cb_offset": 200, "flags": 0}}"#,
    );
    let prog = n.program(&[(&cs, &cj)]);
    n.begin();
    unsafe {
        nvnCommandBufferBindProgram(n.c(), &*prog, 0x20);
        nvnCommandBufferBindStorageBuffer(n.c(), stage::COMPUTE as i32, 0, caddr, 256);
        nvnCommandBufferBindStorageBuffer(n.c(), stage::COMPUTE as i32, 1, daddr, 1024);
        nvnCommandBufferDispatchCompute(n.c(), 4, 1, 1);
        nvnCommandBufferBarrier(n.c(), barrier::ORDER_PRIMITIVES as i32);
    }
    n.submit_wait();
    let got = u32::from_le_bytes(out.mem.slice()[coff..coff + 4].try_into().unwrap());
    eprintln!("ballot counter: {got} counted, {want} wanted");
    assert_eq!(got, want);
    assert_eq!(validation_errors(), e0, "validation errors");
}

/// VRAM eviction (Turbo `vk_budget`): over the budget's threshold (`NXGFX_VRAM_EVICT_PCT`, 0 here so any use is
/// over it) the images parked for a re-initialization are freed.
#[test]
fn vram_budget_frees_parked_images() {
    let _g = setup();
    let mut n = Nvn::new();
    let t = n.texture(256, 256, f::RGBA8, 0, false);
    n.register_texture(11, &t);
    let g = nxgfx::vk::Gpu::get().unwrap();
    // registration alone makes no image (lazy images): its first use does
    g.materialize(11 + n.tex_base());
    let mut t = t;
    unsafe { nxgfx::nvn::tex::nvnTextureFinalize(&mut *t) };
    std::env::set_var("NXGFX_VRAM_EVICT_PCT", "0");
    let freed = g.vram_budget();
    std::env::remove_var("NXGFX_VRAM_EVICT_PCT");
    eprintln!("VRAM budget: {} KB of parked images freed", freed >> 10);
    assert!(freed >= 256 * 256 * 4, "the finalized texture's parked image");
}

/// Ray tracing (`vk/rt.rs`): a bottom level from NVN2's build description of one triangle in guest memory, a top level
/// from an instance record naming it by its guest address, then rays from a 32x32 grid traced against it with ray
/// queries: the half of the grid under the triangle hits, the rest misses.
#[test]
fn ray_query_traces_a_triangle() {
    use nxgfx::nvn::query::{AsBuild, AsGeometry};
    let _g = setup();
    let e0 = validation_errors();
    let mut n = Nvn::new();
    let g = nxgfx::vk::Gpu::get().unwrap();
    if !g.d.caps.rt {
        eprintln!("ray tracing: the device has no acceleration structures, skipped");
        return;
    }
    let tri: [f32; 9] = [-1.0, -1.0, 0.0, 1.0, -1.0, 0.0, -1.0, 1.0, 0.0];
    let va = n.data.put(&tri, 256);
    let (_, blas_at) = n.data.alloc(256, 256);
    let (_, tlas_at) = n.data.alloc(256, 256);
    let mut inst = [0u8; 64];
    for (i, v) in [1.0f32, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0].iter().enumerate() {
        inst[4 * i..4 * i + 4].copy_from_slice(&v.to_le_bytes());
    }
    inst[48..52].copy_from_slice(&(0xffu32 << 24).to_le_bytes());
    inst[56..64].copy_from_slice(&blas_at.to_le_bytes());
    let ia = n.data.put(&inst, 256);
    let tri_geo = AsGeometry { kind: 1, _pad: 0, w: [va, 3 | 34 << 32, 12, 0, 0, 0, 0] };
    let inst_geo = AsGeometry { kind: 3, _pad: 0, w: [ia, 1, 0, 0, 0, 0, 0] };
    let mk = |geo: &AsGeometry, dst: u64| AsBuild { flags: 0, operation: 0, geometries: geo, count: 1, _pad: 0, source: 0, destination: dst, destination_size: 0, scratch: 0, scratch_size: 0 };
    let (bl, tl) = (mk(&tri_geo, blas_at), mk(&inst_geo, tlas_at));
    let f = nxgfx::vk::queue::oneoff(g, |g, sub, _| nxgfx::vk::rt::build(g, sub, &bl).expect("bottom level"));
    nxgfx::vk::queue::wait_fence(f, u64::MAX);
    let f = nxgfx::vk::queue::oneoff(g, |g, sub, _| nxgfx::vk::rt::build(g, sub, &tl).expect("top level"));
    nxgfx::vk::queue::wait_fence(f, u64::MAX);
    let (tlas, _) = nxgfx::vk::rt::lookup(tlas_at).expect("the top level by its guest address");
    // the tracing kernel, with its own layout
    use ash::vk;
    let dev = g.dev();
    let binds = [
        vk::DescriptorSetLayoutBinding::default().binding(0).descriptor_type(vk::DescriptorType::ACCELERATION_STRUCTURE_KHR).descriptor_count(1).stage_flags(vk::ShaderStageFlags::COMPUTE),
        vk::DescriptorSetLayoutBinding::default().binding(1).descriptor_type(vk::DescriptorType::STORAGE_BUFFER).descriptor_count(1).stage_flags(vk::ShaderStageFlags::COMPUTE),
    ];
    let out = g.buffer(32 * 32 * 4, vk::BufferUsageFlags::STORAGE_BUFFER, gpu_allocator::MemoryLocation::GpuToCpu, "rt test out").unwrap();
    unsafe {
        let dsl = dev.create_descriptor_set_layout(&vk::DescriptorSetLayoutCreateInfo::default().bindings(&binds), None).unwrap();
        let sl = [dsl];
        let pl = dev.create_pipeline_layout(&vk::PipelineLayoutCreateInfo::default().set_layouts(&sl), None).unwrap();
        let code = spv("rq_comp");
        let words: Vec<u32> = code.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        let m = dev.create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&words), None).unwrap();
        let st = vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::COMPUTE).module(m).name(c"main");
        let pipe = dev.create_compute_pipelines(vk::PipelineCache::null(), &[vk::ComputePipelineCreateInfo::default().stage(st).layout(pl)], None).unwrap()[0];
        let sizes = [vk::DescriptorPoolSize { ty: vk::DescriptorType::ACCELERATION_STRUCTURE_KHR, descriptor_count: 1 }, vk::DescriptorPoolSize { ty: vk::DescriptorType::STORAGE_BUFFER, descriptor_count: 1 }];
        let pool = dev.create_descriptor_pool(&vk::DescriptorPoolCreateInfo::default().max_sets(1).pool_sizes(&sizes), None).unwrap();
        let set = dev.allocate_descriptor_sets(&vk::DescriptorSetAllocateInfo::default().descriptor_pool(pool).set_layouts(&sl)).unwrap()[0];
        let ases = [tlas];
        let mut asw = vk::WriteDescriptorSetAccelerationStructureKHR::default().acceleration_structures(&ases);
        let bi = [vk::DescriptorBufferInfo { buffer: out.buf, offset: 0, range: vk::WHOLE_SIZE }];
        let mut w0 = vk::WriteDescriptorSet::default().dst_set(set).dst_binding(0).descriptor_type(vk::DescriptorType::ACCELERATION_STRUCTURE_KHR).push_next(&mut asw);
        w0.descriptor_count = 1;
        let w1 = vk::WriteDescriptorSet::default().dst_set(set).dst_binding(1).descriptor_type(vk::DescriptorType::STORAGE_BUFFER).buffer_info(&bi);
        dev.update_descriptor_sets(&[w0, w1], &[]);
        let f = nxgfx::vk::queue::oneoff(g, |g, sub, _| {
            let d = g.dev();
            d.cmd_bind_pipeline(sub.main, vk::PipelineBindPoint::COMPUTE, pipe);
            d.cmd_bind_descriptor_sets(sub.main, vk::PipelineBindPoint::COMPUTE, pl, 0, &[set], &[]);
            d.cmd_dispatch(sub.main, 4, 4, 1);
            nxgfx::vk::queue::full_barrier(g, sub.main);
        });
        nxgfx::vk::queue::wait_fence(f, u64::MAX);
        let hits = core::slice::from_raw_parts(out.ptr as *const u32, 32 * 32);
        let mut bad = 0;
        let mut n_hit = 0;
        for y in 0..32usize {
            for x in 0..32usize {
                let (fx, fy) = ((x as f32 + 0.5) / 16.0 - 1.0, (y as f32 + 0.5) / 16.0 - 1.0);
                // inside the triangle (-1,-1) (1,-1) (-1,1): x + y < 0; the diagonal itself left out
                let want = if (fx + fy).abs() < 0.07 { None } else { Some(fx + fy < 0.0) };
                let got = hits[y * 32 + x] == 1;
                n_hit += got as u32;
                if want.is_some_and(|w| w != got) {
                    bad += 1;
                }
            }
        }
        eprintln!("ray query: 1024 rays, {n_hit} hit the triangle, {bad} wrong");
        assert_eq!(bad, 0, "rays traced against the triangle");
        dev.destroy_pipeline(pipe, None);
        dev.destroy_shader_module(m, None);
        dev.destroy_descriptor_pool(pool, None);
        dev.destroy_pipeline_layout(pl, None);
        dev.destroy_descriptor_set_layout(dsl, None);
    }
    assert_eq!(validation_errors(), e0, "validation errors");
}

/// The loop bound (GPU hangs; `shaders::link::loop_guard`, `NXGFX_LOOP_GUARD`, 65536 back-edges a call): a loop of a
/// million iterations ends after 65,536 of them.
#[test]
fn loops_are_bounded() {
    let _g = setup();
    let e0 = validation_errors();
    let mut n = Nvn::new();
    let mut out = Pool::new(n.d(), 1 << 16, CPU_POOL);
    let (off, addr) = out.alloc(256, 256);
    let ub = n.data.put(&[1_000_000u32, 0, 0, 0], 256);
    let cs = spv("longloop_comp");
    let cj = json(
        "cs",
        r#"{"set": 5, "binding": 1, "type": "storage_buffer", "count": 1, "name": "o", "resource": {"kind": "sbuf", "slot": 0, "cb_slot": 0, "cb_offset": 196, "flags": 1}},
           {"set": 5, "binding": 2, "type": "uniform_buffer", "count": 1, "name": "c3", "resource": {"kind": "cbuf", "slot": 3, "fixed": 0}}"#,
    );
    let prog = n.program(&[(&cs, &cj)]);
    n.begin();
    unsafe {
        nvnCommandBufferBindProgram(n.c(), &*prog, 0x20);
        nvnCommandBufferBindStorageBuffer(n.c(), stage::COMPUTE as i32, 0, addr, 256);
        nvnCommandBufferBindUniformBuffer(n.c(), stage::COMPUTE as i32, 0, ub, 16);
        nvnCommandBufferDispatchCompute(n.c(), 1, 1, 1);
        nvnCommandBufferBarrier(n.c(), barrier::ORDER_PRIMITIVES as i32);
    }
    n.submit_wait();
    let got = u32::from_le_bytes(out.mem.slice()[off..off + 4].try_into().unwrap());
    let cap = nxgfx::vk::shader::loop_cap();
    eprintln!("loop bound: {got} iterations of 1,000,000 (bound {cap})");
    if cap == 0 {
        assert_eq!(got, 1_000_000);
    } else {
        assert!(got < cap + 2 && got + 2 >= cap, "the loop ended at the bound");
    }
    assert_eq!(validation_errors(), e0, "validation errors");
}

/// An indirect dispatch reads its group counts through the clamp (`clamped_indirect`: held to the device's limits
/// before the dispatch, GPU resets): 4 groups of 64 fill 256 words.
#[test]
fn indirect_dispatch_through_the_count_clamp() {
    let _g = setup();
    let e0 = validation_errors();
    let mut n = Nvn::new();
    let mut out = Pool::new(n.d(), 1 << 16, CPU_POOL);
    let (soff, saddr) = out.alloc(1024, 256);
    let ub = n.data.put(&[7u32, 0, 0, 0], 256);
    let args = n.data.put(&[4u32, 1, 1, 0], 256);
    let cs = spv("fill_comp");
    let cj = json(
        "cs",
        r#"{"set": 5, "binding": 1, "type": "storage_buffer", "count": 1, "name": "s0", "resource": {"kind": "sbuf", "slot": 0, "cb_slot": 0, "cb_offset": 196, "flags": 1}},
           {"set": 5, "binding": 2, "type": "uniform_buffer", "count": 1, "name": "c3", "resource": {"kind": "cbuf", "slot": 3, "fixed": 0}}"#,
    );
    let prog = n.program(&[(&cs, &cj)]);
    n.begin();
    unsafe {
        nvnCommandBufferBindProgram(n.c(), &*prog, 0x20);
        nvnCommandBufferBindStorageBuffer(n.c(), stage::COMPUTE as i32, 0, saddr, 1024);
        nvnCommandBufferBindUniformBuffer(n.c(), stage::COMPUTE as i32, 0, ub, 16);
        nvnCommandBufferDispatchComputeIndirect(n.c(), args);
        nvnCommandBufferBarrier(n.c(), barrier::ORDER_PRIMITIVES as i32);
    }
    n.submit_wait();
    let s = &out.mem.slice()[soff..soff + 1024];
    let bad = (0..256).filter(|&i| u32::from_le_bytes(s[4 * i..4 * i + 4].try_into().unwrap()) != i as u32 * 3 + 7).count();
    eprintln!("indirect dispatch: 256 words, {bad} wrong");
    assert_eq!(bad, 0);
    assert_eq!(validation_errors(), e0, "validation errors");
}

/// A plain indirect draw whose record holds garbage counts (vertices x instances in the billions) is held to the draw
/// limits before the pass reads it: the triangle is drawn, the GPU does not work for seconds.
#[test]
fn indirect_draw_with_garbage_counts_is_held() {
    let _g = setup();
    let e0 = validation_errors();
    let mut n = Nvn::new();
    let rt = n.texture(W, H, f::RGBA8, 0, false);
    let tri = tri_program(&mut n);
    let quad: Vec<f32> = [[-1.0f32, -1.0], [3.0, -1.0], [-1.0, 3.0]].iter().flat_map(|p| [p[0], p[1], 0.0, 0.0, 1.0, 0.0, 1.0]).collect();
    let qb = n.data.put(&quad, 256);
    let rec = n.data.put(&[3u32, 0xffff_fff0, 0, 0], 256);
    n.begin();
    n.set_target(&rt, None, W, H);
    clear(&mut n, [0.0, 0.0, 1.0, 1.0]);
    bind_program(&mut n, &tri);
    n.vertex_layout(&[(f::RGB32F, 0), (f::RGBA32F, 12)], 28);
    unsafe {
        nvnCommandBufferBindVertexBuffer(n.c(), 0, qb, quad.len() * 4);
        nvnCommandBufferDrawArraysIndirect(n.c(), prim::TRIANGLES, rec);
    }
    let t0 = std::time::Instant::now();
    n.submit_wait();
    eprintln!("garbage indirect draw: {:.0} ms", t0.elapsed().as_secs_f64() * 1e3);
    let got = n.read_rgba8(&rt, W, H);
    let want: Vec<Option<[u8; 4]>> = (0..W * H).map(|_| Some([0, 255, 0, 255])).collect();
    check("indirect draw with garbage counts", &got, &want, 0);
    assert_eq!(validation_errors(), e0, "validation errors");
}

/// A queue that waits on another queue's sync while that queue is parked at an event wait runs after the parked work
/// (asynchronous replay: the sync's fence named the other queue's op before it had parked, and the wait passed at once:
/// the waiting queue read memory the parked work had not written yet).
#[test]
fn queue_waits_for_another_queues_parked_work() {
    use nxgfx::nvn::queue::*;
    use nxgfx::nvn::sync::*;
    let _g = setup();
    let e0 = validation_errors();
    let mut n = Nvn::new();
    let d = n.d();
    let mut qb = NVNqueueBuilder::boxed();
    let mut qb_obj = NVNqueue::boxed();
    let mut cmd_b = NVNcommandBuffer::boxed();
    let mut ctl_b = vec![0u64; 512];
    let mut cmd_pool_b = Pool::new(d, 1 << 16, CPU_POOL);
    unsafe {
        nvnQueueBuilderSetDevice(&mut *qb, d);
        nvnQueueBuilderSetDefaults(&mut *qb);
        assert_eq!(nvnQueueInitialize(&mut *qb_obj, &*qb), 1);
        nvnCommandBufferInitialize(&mut *cmd_b, d);
        nvnCommandBufferAddCommandMemory(&mut *cmd_b, cmd_pool_b.p(), 0, 1 << 16);
        nvnCommandBufferAddControlMemory(&mut *cmd_b, ctl_b.as_mut_ptr() as *mut u8, ctl_b.len() * 8);
    }
    let (eoff, _) = n.data.alloc(16, 16);
    n.data.mem.slice()[eoff..eoff + 4].fill(0);
    let mut eb = NVNeventBuilder::boxed();
    let mut ev = NVNevent::boxed();
    unsafe {
        nvnEventBuilderSetDefaults(&mut *eb);
        nvnEventBuilderSetStorage(&mut *eb, n.data.p(), eoff as i64);
        assert_eq!(nvnEventInitialize(&mut *ev, &*eb), 1);
    }
    let pattern: Vec<u8> = (0..256).map(|i| (i * 7 + 3) as u8).collect();
    let src = n.data.put(&pattern, 256);
    let (doff, dst) = n.data.alloc(256, 256);
    let (d2off, dst2) = n.data.alloc(256, 256);
    n.data.mem.slice()[doff..doff + 256].fill(0);
    n.data.mem.slice()[d2off..d2off + 256].fill(0);
    let mut sync_a = NVNsync::boxed();
    let mut sync_b = NVNsync::boxed();
    unsafe {
        nvnSyncInitialize(&mut *sync_a, d);
        nvnSyncInitialize(&mut *sync_b, d);
        // A: parked at its event wait, then src -> dst
        nvnCommandBufferBeginRecording(n.c());
        nvnCommandBufferWaitEvent(n.c(), &*ev, event_wait::EQUAL as i32, 1);
        nvnCommandBufferCopyBufferToBuffer(n.c(), src, dst, 256, 0);
        let h = nvnCommandBufferEndRecording(n.c());
        nvnQueueSubmitCommands(&mut *n.queue, 1, &h);
        nvnQueueFenceSync(&mut *n.queue, &mut *sync_a, 0, 0);
        // B: waits for A's sync, then dst -> dst2
        nvnQueueWaitSync(&mut *qb_obj, &*sync_a);
        nvnCommandBufferBeginRecording(&mut *cmd_b);
        nvnCommandBufferCopyBufferToBuffer(&mut *cmd_b, dst, dst2, 256, 0);
        let h = nvnCommandBufferEndRecording(&mut *cmd_b);
        nvnQueueSubmitCommands(&mut *qb_obj, 1, &h);
        nvnQueueFenceSync(&mut *qb_obj, &mut *sync_b, 0, 0);
    }
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert_eq!(unsafe { nvnSyncWait(&*sync_b, 0) }, sync_wait::TIMEOUT_EXPIRED, "queue B ran before A's parked work");
    unsafe {
        nvnEventSignal(&mut *ev, event_signal::WRITE, 1);
        let r = nvnSyncWait(&*sync_b, 5_000_000_000);
        assert!(r != sync_wait::TIMEOUT_EXPIRED, "queue B never ran");
    }
    assert_eq!(&n.data.mem.slice()[d2off..d2off + 256], &pattern[..], "queue B copied before A wrote");
    unsafe {
        nvnCommandBufferFinalize(&mut *cmd_b);
        nvnQueueFinalize(&mut *qb_obj);
    }
    assert_eq!(validation_errors(), e0, "validation errors");
}

/// A sync fenced inside a command buffer and reused every frame covers the new submission once it is submitted: it no
/// longer reads as signalled (last frame's fence) while this frame's work still waits to run.
#[test]
fn reused_command_buffer_sync_covers_the_new_submission() {
    use nxgfx::nvn::queue::*;
    use nxgfx::nvn::sync::*;
    let _g = setup();
    let e0 = validation_errors();
    let mut n = Nvn::new();
    let d = n.d();
    let (eoff, _) = n.data.alloc(16, 16);
    n.data.mem.slice()[eoff..eoff + 4].fill(0);
    let mut eb = NVNeventBuilder::boxed();
    let mut ev = NVNevent::boxed();
    let mut s = NVNsync::boxed();
    unsafe {
        nvnEventBuilderSetDefaults(&mut *eb);
        nvnEventBuilderSetStorage(&mut *eb, n.data.p(), eoff as i64);
        assert_eq!(nvnEventInitialize(&mut *ev, &*eb), 1);
        nvnSyncInitialize(&mut *s, d);
        // last frame: fenced and done
        nvnCommandBufferBeginRecording(n.c());
        nvnCommandBufferFenceSync(n.c(), &mut *s, 0, 0);
        let h = nvnCommandBufferEndRecording(n.c());
        nvnQueueSubmitCommands(&mut *n.queue, 1, &h);
        assert!(nvnSyncWait(&*s, 5_000_000_000) != sync_wait::TIMEOUT_EXPIRED);
        // this frame: held at an event wait before its fence
        nvnCommandBufferBeginRecording(n.c());
        nvnCommandBufferWaitEvent(n.c(), &*ev, event_wait::EQUAL as i32, 1);
        nvnCommandBufferFenceSync(n.c(), &mut *s, 0, 0);
        let h = nvnCommandBufferEndRecording(n.c());
        nvnQueueSubmitCommands(&mut *n.queue, 1, &h);
    }
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert_eq!(unsafe { nvnSyncWait(&*s, 0) }, sync_wait::TIMEOUT_EXPIRED, "the sync read as signalled before its submission ran");
    unsafe {
        nvnEventSignal(&mut *ev, event_signal::WRITE, 1);
        assert!(nvnSyncWait(&*s, 5_000_000_000) != sync_wait::TIMEOUT_EXPIRED, "the sync never signalled");
    }
    assert_eq!(validation_errors(), e0, "validation errors");
}

/// agl's "clear once" (Turbo's render_cond): a clear wrapped in SetRenderEnableConditional(equal) on a record the CPU
/// set to 1 and a reference of 1 at +16, followed by ReportCounter(TIMESTAMP_TOP) on that record: the second wrapped
/// clear of the same target is skipped, since the timestamp's counter field (0) no longer equals the reference, even
/// though the report has not landed yet when the replay reaches the second condition.
#[test]
fn conditional_clear_runs_once_after_a_timestamp_report() {
    use nxgfx::nvn::cmd2::*;
    let _g = setup();
    let e0 = validation_errors();
    let mut n = Nvn::new();
    let rt = n.texture(W, H, f::RGBA8, 0, false);
    let (roff, rec) = n.data.alloc(32, 32);
    {
        let m = n.data.mem.slice();
        m[roff..roff + 32].fill(0);
        m[roff..roff + 8].copy_from_slice(&1u64.to_le_bytes());
        m[roff + 16..roff + 24].copy_from_slice(&1u64.to_le_bytes());
    }
    n.begin();
    n.set_target(&rt, None, W, H);
    clear(&mut n, [0.0, 0.0, 0.0, 1.0]);
    unsafe {
        nvnCommandBufferSetRenderEnableConditional(n.c(), 0, rec);
        nvnCommandBufferClearColor(n.c(), 0, [1.0f32, 0.0, 0.0, 1.0].as_ptr(), 0xf);
        nvnCommandBufferSetRenderEnable(n.c(), 1);
        nvnCommandBufferReportCounter(n.c(), 16, rec);
        nvnCommandBufferSetRenderEnableConditional(n.c(), 0, rec);
        nvnCommandBufferClearColor(n.c(), 0, [0.0f32, 0.0, 1.0, 1.0].as_ptr(), 0xf);
        nvnCommandBufferSetRenderEnable(n.c(), 1);
    }
    n.submit_wait();
    let got = n.read_rgba8(&rt, W, H);
    let want: Vec<Option<[u8; 4]>> = (0..W * H).map(|_| Some([255, 0, 0, 255])).collect();
    check("conditional clear once", &got, &want, 0);
    assert_eq!(&n.data.mem.slice()[roff..roff + 8], &0u64.to_le_bytes(), "the timestamp's counter field");
    assert_eq!(validation_errors(), e0, "validation errors");
}

/// The address binding report (`NXGFX_ADDR_REPORT`) sees the device's memory bindings (a loss names the faulting
/// address's owner from them). Runs only when the report is on.
#[test]
fn address_binding_report_sees_bindings() {
    let _g = setup();
    if !nxgfx::vk::lost::addr_report_wanted() {
        eprintln!("NXGFX_ADDR_REPORT not set: skipped");
        return;
    }
    let mut n = Nvn::new();
    let rt = n.texture(W, H, f::RGBA8, 0, false);
    n.begin();
    n.set_target(&rt, None, W, H);
    clear(&mut n, [0.0, 0.0, 1.0, 1.0]);
    n.submit_wait();
    let (live, _) = nxgfx::vk::lost::binding_counts();
    eprintln!("address binding report: {live} ranges bound");
    assert!(live > 0, "no binding reported");
}

/// A scissor larger than the pass's targets (a game's full-screen scissor over a smaller target) is held to them: the
/// draw covers the target and nothing is written outside it (NVIDIA wrote past the image: Smash's device loss).
#[test]
fn scissor_past_the_target_is_held_to_it() {
    let _g = setup();
    let e0 = validation_errors();
    let mut n = Nvn::new();
    let rt = n.texture(W, H, f::RGBA8, 0, false);
    let prog = tri_program(&mut n);
    n.begin();
    n.set_target(&rt, None, W, H);
    clear(&mut n, [0.0, 0.0, 1.0, 1.0]);
    bind_program(&mut n, &prog);
    unsafe { nvnCommandBufferSetScissor(n.c(), -16, -16, 8192, 8192) };
    draw_tri(&mut n, [[-1.0, -1.0, 0.0], [3.0, -1.0, 0.0], [-1.0, 3.0, 0.0]], [0.0, 1.0, 0.0, 1.0]);
    n.submit_wait();
    let got = n.read_rgba8(&rt, W, H);
    let want: Vec<Option<[u8; 4]>> = (0..W * H).map(|_| Some([0, 255, 0, 255])).collect();
    check("scissor past the target", &got, &want, 0);
    assert_eq!(validation_errors(), e0, "validation errors");
}

/// Frame generation's whole-frame check (`framegen.rs`, `fg_stats.comp`; docs/DECISIONS.md 1): the same frame twice is
/// a duplicate, a completely different one a scene cut followed by two warm-up frames, and a small change is
/// interpolated.
#[test]
fn frame_generation_skips_duplicates_and_scene_cuts() {
    let _g = setup();
    assert!(nxconfig::apply_arg("--lsfg=1"));
    assert!(nxconfig::apply_arg("--frame_gen_type=builtin"));
    assert!(nxconfig::apply_arg("--lsfg_target=0"));
    let mut n = Nvn::new();
    let g = nxgfx::vk::Gpu::get().unwrap();
    let mk = |n: &mut Nvn, rgba: [f32; 4], _: bool| {
        let t = n.texture(W, H, f::RGBA8, 0, false);
        n.begin();
        n.set_target(&t, None, W, H);
        clear(n, rgba);
        n.submit_wait();
        t
    };
    let a = mk(&mut n, [0.2, 0.2, 0.2, 1.0], false);
    let b = mk(&mut n, [0.9, 0.8, 0.1, 1.0], false);
    // a small change: the same picture a little brighter (no pixel changes by a cut's amount)
    let b2 = mk(&mut n, [0.95, 0.85, 0.15, 1.0], false);
    let image = |t: &NVNtexture| {
        let s = unsafe { (*(t as *const NVNtexture as *const nxgfx::nvn::obj::tex_t)).serial };
        g.images.lock().unwrap().by_serial[&s].image
    };
    let frame = |t: &NVNtexture| {
        let img = image(t);
        std::thread::sleep(std::time::Duration::from_millis(16));
        let f = nxgfx::vk::queue::oneoff(g, |g, sub, _| {
            let _ = nxgfx::vk::framegen::process(g, sub, img, 0, 0, W as i32, H as i32);
        });
        assert!(nxgfx::vk::queue::wait_fence(f, 5_000_000_000));
        nxgfx::vk::framegen::last_check().map(|c| c.0)
    };
    frame(&a);
    assert_eq!(frame(&a), Some(1), "the same frame again: a duplicate");
    assert_eq!(frame(&b), Some(2), "a different picture: a scene cut");
    assert_eq!(frame(&b), Some(1), "after the cut, the same again: a duplicate");
    assert_eq!(frame(&b2), Some(3), "warming up after the cut (a small change)");
    assert_eq!(frame(&b), Some(3), "warming up, the second frame");
    assert_eq!(frame(&b2), Some(0), "a small change once warm: interpolated");
    assert!(nxconfig::apply_arg("--lsfg=0"));
}

/// A texture object whose image a newer object over the same storage took over (Mario Kart 8 Deluxe makes new objects
/// over its render targets every frame while it still registers the old one into new pool slots) is found through the
/// newer one: the slot naming the old object binds the image, not the zero stand-in (its black lap stretches).
#[test]
fn a_taken_over_texture_object_still_finds_its_image() {
    let _g = setup();
    let mut n = Nvn::new();
    let (w, h) = (32u32, 16u32);
    let a = n.texture(w, h, f::RGBA8, 0, false);
    let g = nxgfx::vk::Gpu::get().unwrap();
    let ta = unsafe { *(&*a as *const NVNtexture as *const nxgfx::nvn::obj::tex_t) };
    {
        let mut imgs = g.images.lock().unwrap();
        assert!(g.ensure_image(&mut imgs, &ta));
        // drawn by the GPU (what a render target is)
        imgs.by_serial.get_mut(&ta.serial).unwrap().gpu_owned = true;
    }
    // a new object over the same storage and description
    let b = unsafe {
        let mut tb = NVNtextureBuilder::boxed();
        let mut t = NVNtexture::boxed();
        nvnTextureBuilderSetDevice(&mut *tb, n.d());
        nvnTextureBuilderSetDefaults(&mut *tb);
        nvnTextureBuilderSetSize2D(&mut *tb, w as i32, h as i32);
        nvnTextureBuilderSetFormat(&mut *tb, f::RGBA8);
        nvnTextureBuilderSetStorage(&mut *tb, nvnTextureGetMemoryPool(&*a), nvnTextureGetMemoryOffset(&*a));
        assert_eq!(nvnTextureInitialize(&mut *t, &*tb), 1);
        t
    };
    let tb_ = unsafe { *(&*b as *const NVNtexture as *const nxgfx::nvn::obj::tex_t) };
    let mut imgs = g.images.lock().unwrap();
    let image_a = imgs.by_serial[&ta.serial].image;
    assert!(g.ensure_image(&mut imgs, &tb_));
    assert!(!imgs.by_serial.contains_key(&ta.serial), "the newer object took the image over");
    assert_eq!(imgs.by_serial[&tb_.serial].image, image_a, "with its content");
    assert_eq!(imgs.live(ta.serial), tb_.serial, "the older object finds it through the newer one");
    assert_eq!(imgs.live(tb_.serial), tb_.serial);
    // the older object made again takes it back: no forwarding left for it
    assert!(g.ensure_image(&mut imgs, &ta));
    assert_eq!(imgs.live(ta.serial), ta.serial);
    assert_eq!(imgs.live(tb_.serial), ta.serial);
}

/// Block-linear storage deswizzled by the GPU (`tex::deswizzle_gpu`) gives the CPU deswizzle's rows, byte for byte:
/// mips, array layers, a 3D texture with deep blocks, compressed formats, and both GOB orders (Switch, Switch 2).
#[test]
fn block_linear_textures_deswizzle_on_the_gpu_as_on_the_cpu() {
    use nxgfx::tex::deswizzle::{deswizzle, geom, GOB_SWITCH2};
    use nxgfx::tex::layout::{tex_layout, TexDesc};
    let _g = setup();
    let e0 = validation_errors();
    let _n = Nvn::new();
    let g = nxgfx::vk::Gpu::get().unwrap();
    let mut levels_checked = 0;
    for s2 in [false, true] {
        GOB_SWITCH2.store(s2, std::sync::atomic::Ordering::Relaxed);
        // (format, width, height, depth, levels, target): RGBA8 2D with mips, an RGBA8 array, R8 (rows of 1-byte
        // texels), BC1 and BC7 (8- and 16-byte blocks), RGBA16F 3D
        for (fmt, w, h, depth, levels, target) in [(0x25, 300, 200, 1, 5, 1), (0x25, 64, 48, 6, 3, 4), (0x01, 516, 37, 1, 2, 1), (0x42, 256, 128, 1, 4, 1), (0x49, 96, 160, 1, 3, 1), (0x29, 40, 24, 20, 2, 2)] {
            let d = TexDesc { target, format: fmt, width: w, height: h, depth, levels, ..Default::default() };
            let l = tex_layout(&d);
            let storage: Vec<u8> = (0..l.size as u32).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
            for layer in 0..if target == 2 { 1 } else { l.layers } {
                for lv in 0..levels {
                    let gm = geom(&d, lv, layer).unwrap();
                    if gm.row_bytes % 4 != 0 {
                        continue;
                    }
                    let n = gm.row_bytes as usize * gm.rows as usize * gm.slices as usize;
                    let mut want = vec![0u8; n];
                    deswizzle(&d, lv, layer, &storage, &mut want, gm.row_bytes as usize).unwrap();
                    let got = nxgfx::tex::deswizzle_gpu::deswizzle_on_gpu(g, &d, lv, layer, &storage).expect("GPU deswizzle");
                    assert!(got == want, "format {fmt:#x} {w}x{h}x{depth} level {lv} layer {layer} switch2 {s2}");
                    levels_checked += 1;
                }
            }
        }
    }
    GOB_SWITCH2.store(false, std::sync::atomic::Ordering::Relaxed);
    eprintln!("{levels_checked} levels / layers deswizzled on the GPU, all equal to the CPU's");
    assert_eq!(validation_errors(), e0, "validation errors");
}

/// A texture the CPU writes (nvnTextureWriteTexels: its storage) is uploaded through the GPU deswizzle and reads back
/// as written.
#[test]
fn texture_upload_goes_through_the_gpu_deswizzle() {
    let _g = setup();
    let e0 = validation_errors();
    let mut n = Nvn::new();
    let (w, h) = (512u32, 384u32);
    let tex = n.texture(w, h, f::RGBA8, 0, true);
    let src: Vec<u8> = (0..w * h * 4).map(|i| (i.wrapping_mul(2246822519) >> 11) as u8).collect();
    let before = nxgfx::vk::image::GPU_DESWIZZLED.load(std::sync::atomic::Ordering::Relaxed);
    unsafe {
        let r = NVNcopyRegion { xoffset: 0, yoffset: 0, zoffset: 0, width: w as i32, height: h as i32, depth: 1 };
        nvnTextureWriteTexels(&*tex, core::ptr::null(), &r, src.as_ptr());
    }
    n.begin();
    n.submit_wait();
    let got = n.read_rgba8(&tex, w, h);
    assert!(got == src, "texels read back as written");
    let moved = nxgfx::vk::image::GPU_DESWIZZLED.load(std::sync::atomic::Ordering::Relaxed) - before;
    assert!(moved >= (w * h * 4) as u64, "the upload took the GPU deswizzle ({moved} bytes)");
    assert_eq!(validation_errors(), e0, "validation errors");
}

/// F5's graphics part lists the render targets that exist (a 64x64 RGBA8 target drawn into here) and asks for a pass
/// dump of the next frame.
#[test]
fn diagnostic_dump_lists_the_render_targets() {
    let _g = setup();
    let e0 = validation_errors();
    let mut n = Nvn::new();
    let rt = n.texture(W, H, f::RGBA8, 0, false);
    n.begin();
    n.set_target(&rt, None, W, H);
    clear(&mut n, [0.0, 0.5, 0.0, 1.0]);
    n.submit_wait();
    let d = nxgfx::diag_dump();
    eprintln!("{d}");
    assert!(d.starts_with("# render targets ("), "{d}");
    assert!(d.lines().any(|l| l.contains("64x64x1") && l.contains("R8G8B8A8")), "the target is listed: {d}");
    assert_eq!(validation_errors(), e0, "validation errors");
}

/// A memory pool larger than the device's largest buffer (here made 1 MiB: `mem::MAX_BUFFER_OVERRIDE`, as AMD's 2 GiB
/// against a 3.4 GiB pool) gets several buffers side by side: copies inside each window, between windows, the CPU's
/// writes uploaded into the right window and the GPU's written back from it.
#[test]
fn pool_larger_than_the_largest_buffer_has_windows() {
    let _g = setup();
    let e0 = validation_errors();
    let mut n = Nvn::new();
    nxgfx::vk::mem::MAX_BUFFER_OVERRIDE.store(1 << 20, std::sync::atomic::Ordering::Relaxed);
    let size = (3 << 20) + (512 << 10);
    let big = Pool::new(n.d(), size, CPU_POOL);
    let base = big.mem.addr();
    let pat = |seed: usize| -> Vec<u8> { (0..4096).map(|i| ((i * 31 + seed * 7) ^ (i >> 5)) as u8).collect() };
    // window 0 at +0x100, window 2 at +2.5 MiB, written before the pool's first GPU use
    big.mem.slice()[0x100..0x100 + 4096].copy_from_slice(&pat(1));
    big.mem.slice()[(5 << 19)..(5 << 19) + 4096].copy_from_slice(&pat(2));
    n.begin();
    unsafe {
        nvnCommandBufferCopyBufferToBuffer(n.c(), base + 0x100, base + (3 << 19), 4096, 0); // window 0 -> window 1
        nvnCommandBufferCopyBufferToBuffer(n.c(), base + (5 << 19), base + 0x2000, 4096, 0); // window 2 -> window 0
    }
    n.submit_wait();
    let s = big.mem.slice();
    assert!(s[(3 << 19)..(3 << 19) + 4096] == pat(1)[..], "window 0 -> window 1");
    assert!(s[0x2000..0x2000 + 4096] == pat(2)[..], "window 2 -> window 0");
    // the CPU writes window 3 after the pool is on the GPU; a copy carries it into window 1
    big.mem.slice()[(3 << 20) + 0x40..(3 << 20) + 0x40 + 4096].copy_from_slice(&pat(3));
    n.begin();
    unsafe { nvnCommandBufferCopyBufferToBuffer(n.c(), base + (3 << 20) + 0x40, base + (1 << 20) + 0x800, 4096, 0) };
    n.submit_wait();
    assert!(big.mem.slice()[(1 << 20) + 0x800..(1 << 20) + 0x800 + 4096] == pat(3)[..], "a CPU write in window 3 reaches window 1");
    nxgfx::vk::mem::MAX_BUFFER_OVERRIDE.store(0, std::sync::atomic::Ordering::Relaxed);
    drop(big);
    assert_eq!(validation_errors(), e0, "validation errors");
}

/// A memory pool of 8 MiB and up is a sparse-residency buffer backed in 2 MiB granules on first use: copies inside
/// two granules bind those two only (with the guest bytes there), the rest of the pool holds no device memory.
#[test]
fn large_pools_are_backed_in_granules_on_first_use() {
    let _g = setup();
    let e0 = validation_errors();
    let mut n = Nvn::new();
    let g = nxgfx::vk::Gpu::get().unwrap();
    if !g.d.caps.sparse_buffer {
        eprintln!("no sparse residency buffers on this device: skipped");
        return;
    }
    let size = 64 << 20;
    let big = Pool::new(n.d(), size, CPU_POOL);
    let base = big.mem.addr();
    let pat: Vec<u8> = (0..65536).map(|i| (i * 13 ^ (i >> 7)) as u8).collect();
    big.mem.slice()[(10 << 20) + 0x40..(10 << 20) + 0x40 + 65536].copy_from_slice(&pat);
    n.begin();
    unsafe { nvnCommandBufferCopyBufferToBuffer(n.c(), base + (10 << 20) + 0x40, base + (40 << 20) + 0x100, 65536, 0) };
    n.submit_wait();
    assert!(big.mem.slice()[(40 << 20) + 0x100..(40 << 20) + 0x100 + 65536] == pat[..], "copied through two granules");
    let bound = nxgfx::vk::mem::pool_bound(base).expect("a pool backed in granules");
    eprintln!("64 MiB pool: {} MiB bound", bound >> 20);
    assert!(bound <= 4 << 20 && bound >= 2 << 20, "two granules bound, not the pool: {bound}");
    // a CPU write into a bound granule after its first use reaches the GPU; one into an unbound granule comes with it
    big.mem.slice()[(10 << 20) + 0x40..(10 << 20) + 0x40 + 4].copy_from_slice(&[1, 2, 3, 4]);
    big.mem.slice()[(60 << 20)..(60 << 20) + 4].copy_from_slice(&[9, 8, 7, 6]);
    n.begin();
    unsafe {
        nvnCommandBufferCopyBufferToBuffer(n.c(), base + (10 << 20) + 0x40, base + (40 << 20), 4, 0);
        nvnCommandBufferCopyBufferToBuffer(n.c(), base + (60 << 20), base + (40 << 20) + 8, 4, 0);
    }
    n.submit_wait();
    assert_eq!(&big.mem.slice()[(40 << 20)..(40 << 20) + 12], &[1, 2, 3, 4, 0, 0, 0, 0, 9, 8, 7, 6]);
    drop(big);
    assert_eq!(validation_errors(), e0, "validation errors");
}

/// The per-frame upload budget: CPU writes past it wait in their pool; the one a copy of this submission reads goes
/// at once (its use needs it), the others with the next submissions, every byte in the end.
#[test]
fn upload_budget_defers_cpu_writes_no_use_needs_yet() {
    use std::sync::atomic::Ordering::Relaxed;
    let _g = setup();
    let e0 = validation_errors();
    let mut n = Nvn::new();
    let big = Pool::new(n.d(), 4 << 20, CPU_POOL);
    let base = big.mem.addr();
    // the pool on the GPU first
    n.begin();
    unsafe { nvnCommandBufferCopyBufferToBuffer(n.c(), base, base + 0x1000, 16, 0) };
    n.submit_wait();
    nxgfx::vk::mem::BUDGET_OVERRIDE.store(64 << 10, Relaxed);
    let (d0, u0) = (nxgfx::vk::mem::DEFERRED_BYTES.load(Relaxed), nxgfx::vk::mem::DEFERRED_USED.load(Relaxed));
    // four 256 KiB regions written by the CPU: over a 64 KiB budget
    for k in 0..4usize {
        let o = (k + 1) << 19;
        for (i, x) in big.mem.slice()[o..o + (256 << 10)].iter_mut().enumerate() {
            *x = (i * 3 + k * 50) as u8;
        }
    }
    // this submission reads region 3 only
    n.begin();
    unsafe { nvnCommandBufferCopyBufferToBuffer(n.c(), base + (4 << 19), base + 0x10000, 4096, 0) };
    n.submit_wait();
    let want = |k: usize, i: usize| (i * 3 + k * 50) as u8;
    assert!((0..4096).all(|i| big.mem.slice()[0x10000 + i] == want(3, i)), "the region a copy reads is uploaded for it");
    let deferred = nxgfx::vk::mem::DEFERRED_BYTES.load(Relaxed) - d0;
    let used = nxgfx::vk::mem::DEFERRED_USED.load(Relaxed) - u0;
    eprintln!("budget 64 KiB: {} KiB left for later, {} KiB of it taken by a use", deferred >> 10, used >> 10);
    assert!(deferred >= 512 << 10, "writes past the budget waited: {deferred}");
    assert!(used >= 4096, "the used one went at once");
    // later submissions carry the rest within their budgets: each region reads back right
    nxgfx::vk::mem::BUDGET_OVERRIDE.store(0, Relaxed);
    for k in 0..3usize {
        n.begin();
        unsafe { nvnCommandBufferCopyBufferToBuffer(n.c(), base + (((k + 1) << 19) as u64) + 0x100, base + 0x20000 + (k as u64) * 0x1000, 256, 0) };
        n.submit_wait();
        assert!((0..256).all(|i| big.mem.slice()[0x20000 + k * 0x1000 + i] == want(k, 0x100 + i)), "region {k}");
    }
    drop(big);
    assert_eq!(validation_errors(), e0, "validation errors");
}
