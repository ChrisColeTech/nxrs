//! Headless rendering through the NVN API, each frame compared with the image it must give, with the Vulkan
//! validation layer on and no message allowed. `cargo test -p graphics --features gpu-tests -- --test-threads=1`.
#![cfg(feature = "gpu-tests")]

mod common;
use common::*;
use nxgfx::nvn::cmd::*;

use nxgfx::nvn::state::*;
use nxgfx::nvn::types::*;

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
