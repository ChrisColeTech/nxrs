//! Deferred recording (`NXGFX_DEFER=1`, vk/defer.rs) in a process of its own: the replay's commands for the submission
//! go through the recorder thread and the picture is what the direct path draws.
#![cfg(feature = "gpu-tests")]

mod common;
use common::*;
use nxgfx::nvn::cmd::*;
use nxgfx::nvn::types::*;

#[test]
fn deferred_recording_draws_the_same() {
    std::env::set_var("NXGFX_DEFER", "1");
    let _g = setup();
    let e0 = validation_errors();
    let mut n = Nvn::new();
    assert!(
        nxgfx::vk::defer::active(),
        "the shims are in the device's tables"
    );
    let rt = n.texture(64, 64, 37, 0, false);
    let vs = spv("tri_vert");
    let fs = spv("tri_frag");
    let prog = n.program(&[(&vs, &json("vs", "")), (&fs, &json("ps", ""))]);
    let q0 = nxgfx::vk::defer::QUEUED.load(std::sync::atomic::Ordering::Relaxed);
    n.begin();
    n.set_target(&rt, None, 64, 64);
    unsafe { nvnCommandBufferClearColor(n.c(), 0, [0.0f32, 0.0, 1.0, 1.0].as_ptr(), 0xf) };
    unsafe { nvnCommandBufferBindProgram(n.c(), &*prog, 0x1f) };
    // 200 small triangles: many queued state and draw commands
    for i in 0..200 {
        let x = (i % 20) as f32 / 10.0 - 1.0;
        let y = (i / 20) as f32 / 5.0 - 1.0;
        let mut data = Vec::new();
        for p in [[x, y, 0.0f32], [x + 0.1, y, 0.0], [x, y + 0.2, 0.0]] {
            data.extend_from_slice(&p);
            data.extend_from_slice(&[1.0f32, 0.0, 0.0, 1.0]);
        }
        let a = n.data.put(&data, 256);
        n.vertex_layout(&[(34, 0), (46, 12)], 28);
        unsafe {
            nvnCommandBufferBindVertexBuffer(n.c(), 0, a, data.len() * 4);
            nvnCommandBufferDrawArrays(n.c(), prim::TRIANGLES, 0, 3);
        }
    }
    n.submit_wait();
    let queued = nxgfx::vk::defer::QUEUED.load(std::sync::atomic::Ordering::Relaxed) - q0;
    let got = n.read_rgba8(&rt, 64, 64);
    let red = got.chunks(4).filter(|p| p == &[255, 0, 0, 255]).count();
    let blue = got.chunks(4).filter(|p| p == &[0, 0, 255, 255]).count();
    eprintln!("deferred recording: {queued} commands queued, {red} red / {blue} blue pixels");
    assert!(queued > 400, "the draws went through the recorder");
    assert!(
        red > 200 && blue > 1000 && red + blue == 64 * 64,
        "the triangles over the clear"
    );
    assert_eq!(validation_errors(), e0, "validation errors");
}
