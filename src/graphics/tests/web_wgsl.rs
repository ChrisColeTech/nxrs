//! The web build's GPU (docs/35 W4) turns every module the replay makes into WGSL (`shaders::wgsl`). The replay's own
//! compute kernels a title can reach on the web are converted here, as the web device does at vkCreateShaderModule.
//! (`clamp_counts` reads through buffer device addresses: the replay builds it only on a device that has them, which
//! the web device does not; `exec.rs` `clamp_pipe`.)

fn words(b: &[u8]) -> Vec<u32> {
    b.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

#[test]
fn the_replays_own_kernels_become_wgsl() {
    let kernels: [(&str, &[u8]); 4] = [
        ("deswizzle", include_bytes!(concat!(env!("OUT_DIR"), "/spv/deswizzle_comp.spv"))),
        ("astc_decode", include_bytes!(concat!(env!("OUT_DIR"), "/spv/astc_decode_comp.spv"))),
        ("bc7_encode", include_bytes!(concat!(env!("OUT_DIR"), "/spv/bc7_encode_comp.spv"))),
        ("bc7_encode_m6", include_bytes!(concat!(env!("OUT_DIR"), "/spv/bc7_encode_comp_m6.spv"))),
    ];
    let mut bad = Vec::new();
    for (name, spv) in kernels {
        match shaders::wgsl::convert(&words(spv)) {
            Ok(w) => {
                assert_eq!(w.stage, shaders::wgsl::Stage::Compute, "{name}");
                assert!(w.code.starts_with("diagnostic(off, derivative_uniformity);"), "{name}");
            }
            Err(e) => bad.push(format!("{name}: {e}")),
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}
