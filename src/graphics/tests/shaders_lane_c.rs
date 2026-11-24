//! Lane C's real shaders in the shared layout: the program pairs of a game's shader build (its `pairs.tsv` and
//! `spv/<key>.{spv,json}`), each module remapped from its reflection into the one pipeline layout and built into a
//! pipeline with the validation layer on; then the pipeline key file they left, rebuilt by the prewarm within a
//! memory budget. Uses `NXGFX_TEST_SHADERS` (default: the workspace's `build/shaders/mk8d`); skipped when absent.
#![cfg(feature = "gpu-tests")]

mod common;
use nxgfx::nvn::prog::ShaderKey;
use nxgfx::vk::pipe::GfxKey;
use std::path::PathBuf;

fn key(h: &str) -> Option<ShaderKey> {
    if h.len() != 16 {
        return None;
    }
    let mut k = [0u8; 8];
    for i in 0..8 {
        k[i] = u8::from_str_radix(&h[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(k)
}

fn dir() -> Option<PathBuf> {
    let d = std::env::var("NXGFX_TEST_SHADERS")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../build/shaders/mk8d")
        });
    d.join("pairs.tsv").exists().then_some(d)
}

#[test]
fn lane_c_shaders_build_pipelines_in_the_shared_layout() {
    let _l = common::setup();
    let Some(d) = dir() else {
        eprintln!("lane C shaders: no shader build found, skipped");
        return;
    };
    let n = common::Nvn::new();
    let e0 = common::validation_errors();
    let g = nxgfx::vk::Gpu::get().unwrap();
    g.shaders.set_dir(d.clone());
    let cache = std::env::temp_dir().join(format!("nxgfx-test-cache-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&cache);
    g.pipes.set_dir(cache.clone());
    let pairs = std::fs::read_to_string(d.join("pairs.tsv")).unwrap();
    let want: usize = std::env::var("NXGFX_TEST_PAIRS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(400);
    let (mut tried, mut built, mut no_module) = (0, 0, 0);
    let mut modules = std::collections::HashSet::new();
    let base = nxgfx::vk::pipe::private_bytes();
    let t0 = std::time::Instant::now();
    for line in pairs.lines().filter(|l| !l.starts_with('#')) {
        if tried >= want {
            break;
        }
        let f: Vec<&str> = line.split('\t').collect();
        let (Some(vk), Some(pk)) = (
            f.first().and_then(|h| key(h)),
            f.get(1).and_then(|h| key(h)),
        ) else {
            continue;
        };
        tried += 1;
        let (Some(vm), Some(pm)) = (g.shaders.get(g.dev(), &vk), g.shaders.get(g.dev(), &pk))
        else {
            no_module += 1;
            continue;
        };
        modules.insert(vk);
        modules.insert(pk);
        let mut k = GfxKey {
            ncolor: 4,
            depth: ash::vk::Format::D24_UNORM_S8_UINT.as_raw(),
            samples: 1,
            ..Default::default()
        };
        k.shaders[0] = vk;
        k.shaders[1] = pk;
        for c in 0..4 {
            k.colors[c] = ash::vk::Format::R8G8B8A8_UNORM.as_raw();
        }
        if g.gfx_pipeline(&k, &[vm, pm]).is_some() {
            built += 1;
        }
    }
    let secs = t0.elapsed().as_secs_f64();
    let growth = nxgfx::vk::pipe::private_bytes().saturating_sub(base);
    // compute shaders too
    let manifest = std::fs::read_to_string(d.join("manifest.tsv")).unwrap_or_default();
    let (mut cs_tried, mut cs_built) = (0, 0);
    for line in manifest
        .lines()
        .filter(|l| !l.starts_with('#') && !l.starts_with("key"))
    {
        let f: Vec<&str> = line.split('\t').collect();
        if f.get(1) != Some(&"cs") || f.get(4) != Some(&"ok") || cs_tried >= 100 {
            continue;
        }
        let Some(ck) = key(f[0]) else { continue };
        cs_tried += 1;
        if let Some(m) = g.shaders.get(g.dev(), &ck) {
            if g.compute_pipeline(&m).is_some() {
                cs_built += 1;
            }
        }
    }
    let errs = common::validation_errors() - e0;
    eprintln!(
        "lane C shaders: {tried} program pairs, {built} pipelines built ({no_module} with a stage the shared layout cannot bind yet), {} modules; {cs_built} of {cs_tried} compute pipelines; {secs:.2} s, private memory +{} MB ({:.0} KB a pipeline); {errs} validation errors",
        modules.len(),
        growth >> 20,
        growth as f64 / 1024.0 / (built + cs_built).max(1) as f64
    );
    assert!(
        built * 10 >= tried * 9,
        "most pairs build: {built} of {tried}"
    );
    assert_eq!(errs, 0, "validation errors");

    // the key file those pipelines left: rebuilt by the prewarm, then cut short by a budget of zero
    let keys_written = g.pipes.count();
    g.pipes.clear(g);
    let r = nxgfx::vk::pipe::prewarm(g, 4);
    eprintln!(
        "prewarm: {} keys, {} built, {} skipped, +{} MB, {:.2} s",
        r.keys,
        r.built,
        r.skipped,
        r.private_growth >> 20,
        r.secs
    );
    assert_eq!(
        r.keys, keys_written,
        "every built pipeline's key is in the key file"
    );
    assert_eq!(r.built, r.keys, "the prewarm rebuilds them all");
    g.pipes.clear(g);
    std::env::set_var("NXGFX_PREWARM_BUDGET_MB", "0");
    std::env::set_var("NXGFX_PREWARM_MAX", "5");
    let r = nxgfx::vk::pipe::prewarm(g, 1);
    std::env::remove_var("NXGFX_PREWARM_BUDGET_MB");
    std::env::remove_var("NXGFX_PREWARM_MAX");
    eprintln!(
        "prewarm with a zero budget / 5 pipeline cap: {} of {} built, stopped {}",
        r.built, r.keys, r.stopped_by_budget
    );
    assert!(
        r.stopped_by_budget && r.built <= 5,
        "the budget stops the prewarm"
    );
    assert_eq!(common::validation_errors() - e0, 0, "validation errors");
    drop(n);
    let _ = std::fs::remove_dir_all(&cache);
}

/// One graphics program of a game's shader build into a pipeline (`NXGFX_TEST_SHADERS` = the game's `_data/shaders`,
/// `NXGFX_TEST_PROGRAM` = its stage keys, including vertex and pixel; `NXGFX_TEST_PAIR` remains the two-stage form):
/// a validation or driver compiler failure reproduced outside the game.
#[test]
#[ignore]
fn one_program_pair_builds() {
    let _l = common::setup();
    let e0 = common::validation_errors();
    let Ok(d) = std::env::var("NXGFX_TEST_SHADERS") else {
        eprintln!("one graphics program: NXGFX_TEST_SHADERS not set, skipped");
        return;
    };
    let program = std::env::var("NXGFX_TEST_PROGRAM").or_else(|_| std::env::var("NXGFX_TEST_PAIR"));
    let Ok(program) = program else {
        eprintln!("one graphics program: NXGFX_TEST_PROGRAM / NXGFX_TEST_PAIR not set, skipped");
        return;
    };
    let _n = common::Nvn::new();
    let g = nxgfx::vk::Gpu::get().unwrap();
    g.shaders.set_dir(PathBuf::from(d));
    let ks: Vec<ShaderKey> = program.split_whitespace().filter_map(key).collect();
    assert!(ks.len() >= 2, "at least vertex and pixel keys");
    let mods = ks
        .iter()
        .map(|k| g.shaders.get(g.dev(), k).expect("shader module"))
        .collect::<Vec<_>>();
    let mut k = GfxKey {
        ncolor: 4,
        depth: ash::vk::Format::D24_UNORM_S8_UINT.as_raw(),
        samples: 1,
        ..Default::default()
    };
    for (key, module) in ks.iter().zip(&mods) {
        k.shaders[module.stage as usize] = *key;
    }
    assert_ne!(k.shaders[0], [0; 8], "vertex stage");
    assert_ne!(k.shaders[1], [0; 8], "pixel stage");
    for c in 0..4 {
        k.colors[c] = ash::vk::Format::R8G8B8A8_UNORM.as_raw();
    }
    eprintln!("building the pipeline");
    assert!(g.gfx_pipeline(&k, &mods).is_some(), "pipeline built");
    assert_eq!(common::validation_errors() - e0, 0, "validation errors");
    eprintln!("built");
}
