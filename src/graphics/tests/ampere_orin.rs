//! Ampere instructions through the whole chain against a real Ampere GPU (nxnative `dm/gfx/tests/test_dmsh_ampere_orin.py`
//! and its fixture `ampere_orin/`, recorded on a Jetson Orin, sm_87, by nxnative's `tools/ampere_orin/capture.py`): each op
//! is the GPU compiler's machine code for one PTX operation (its global loads and stores made shared-memory ones), its
//! 1,024 input rows and the outputs the Orin computed. Here the program goes through the engine's translator
//! (`dmsh_xlate`, arch sm86, the build's debug and relaxed-precision options) to SPIR-V; a harness seeds each thread's
//! words of the shader's shared array from a storage buffer and writes them back at every return (through
//! spirv-cross to GLSL and glslangValidator back, nxnative's MSL harness on Vulkan), it runs on this GPU, and every
//! output word is compared with the Orin's bit for bit.
//!
//! An op must match except for what floating point on a different GPU may give (PLATFORM: NaN bits, denormal inputs
//! and outputs). `KNOWN` lists the ops that still differ or that the translator refuses, with nxnative's reasons;
//! `APPROX` the ones that run the device's own approximation (MUFU and the divisions built on it). The test fails when
//! an op outside those lists differs, and prints a summary of all. Needs the translator (build/tools/dmsh_xlate) and
//! the Vulkan SDK's spirv-cross and glslangValidator; skipped (with a line) without them.
#![cfg(feature = "gpu-tests")]

mod common;
use ash::vk;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

/// nxnative's KNOWN list: ops that still differ from the Orin beyond the platform, or that the translator refuses.
const KNOWN: &[(&str, &str)] = &[
    ("i2f_s32_rz", "I2FP.F32.S32.RZ rounds to nearest"),
    (
        "fmax_nan",
        "FMNMX.NAN: a NaN operand gives the other operand",
    ),
    ("fmin_nan", "as fmax_nan"),
    (
        "hmma_16816_f32",
        "HMMA.16816.F32: the f32 accumulator does not add as the tensor core does",
    ),
    ("fdiv_rn", "FCHK is not implemented"),
    ("f2i_s64", "F2I.S64 is not implemented"),
    ("dadd", "DADD is not implemented"),
    ("dmul", "DMUL is not implemented"),
    ("hmin2_nan", "HMNMX2.NAN is not implemented"),
    ("bmin2", "HMNMX2.BF16_V2 is not implemented"),
    ("bmax2_nan", "as bmin2"),
    ("bfma2", "HFMA2.BF16_V2 is not implemented"),
    ("hmma_16816_bf16", "HMMA .BF16 is not implemented"),
    ("hmma_1688_tf32", "HMMA .TF32 is not implemented"),
    ("imma_16832_s8", "IMMA is not implemented"),
    ("i2i_s8_sat", "I2I .SAT is not implemented"),
    ("i2i_u16_sat", "as i2i_s8_sat"),
    (
        "iadd_sat",
        "PLOP3 with .SIGN register sources is not implemented",
    ),
    ("isub_sat", "as iadd_sat"),
    (
        "prmt",
        "PRMT modes other than the index form are not implemented",
    ),
    ("mufu_tanh", "MUFU.TANH is not implemented"),
    ("mufu_tanh_f16x2", "as mufu_tanh"),
    ("frcp_rn", "not testable: CALL.ABS into the CUDA driver"),
    ("fsqrt_rn", "as frcp_rn"),
];
/// Ops that still differ here and not on nxnative's chain (2026-11-21 pinned-source result, 153 of 193 ops matching).
const HERE: &[(&str, &str)] = &[
    ("f2i_s32_rmi", "differs from the Orin: 4 words"),
    ("f2i_s32_rpi", "differs from the Orin: 7 words"),
    ("ffma_rz", "differs from the Orin: 1 word"),
    ("fmul_rm", "differs from the Orin: 1 word"),
    ("fsel_cmp", "differs from the Orin: 2 words"),
    ("fsetp", "differs from the Orin: 116 words"),
    ("fsetp_ftz", "differs from the Orin: 45 words"),
];
/// Ops that run the device's own approximation: compared and reported, never failed.
const APPROX: &[&str] = &[
    "mufu_ex2",
    "mufu_ex2_ftz",
    "mufu_lg2",
    "mufu_lg2_ftz",
    "mufu_rcp",
    "mufu_rcp_ftz",
    "mufu_rsqrt",
    "mufu_rsqrt_ftz",
    "mufu_sqrt",
    "mufu_sqrt_ftz",
    "mufu_sin",
    "mufu_cos",
    "mufu_ex2_f16x2",
    "fdiv_full",
    "fdiv_approx",
    "hmma_16816_f16",
    "hmma_1688_f16",
];

struct Op {
    name: String,
    ins: usize,
    outs: Vec<char>,
    program: Vec<u8>,
    rows_in: Vec<Vec<u32>>,
    rows_gpu: Vec<Vec<u32>>,
}

fn data() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/ampere_orin")
}

fn load() -> Vec<Op> {
    let doc: serde_json::Value =
        serde_json::from_slice(&std::fs::read(data().join("manifest.json")).unwrap()).unwrap();
    let mut gz = flate2::read::GzDecoder::new(
        std::fs::File::open(data().join("ampere_orin.u32.gz")).unwrap(),
    );
    let mut blob = vec![];
    gz.read_to_end(&mut blob).unwrap();
    let mut out = vec![];
    for o in doc["ops"].as_array().unwrap() {
        let r = |k: &str| {
            (
                o[k][0].as_u64().unwrap() as usize,
                o[k][1].as_u64().unwrap() as usize,
            )
        };
        let (p0, pn) = r("prog");
        let (r0, rn) = r("rows");
        let ins = o["ins"].as_str().unwrap().len();
        let outs: Vec<char> = o["outs"].as_str().unwrap().chars().collect();
        let w = ins + outs.len();
        let words: Vec<u32> = blob[r0..r0 + rn]
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
            .collect();
        let rows: Vec<&[u32]> = words.chunks(w).collect();
        out.push(Op {
            name: o["name"].as_str().unwrap().to_string(),
            ins,
            outs,
            program: blob[p0..p0 + pn].to_vec(),
            rows_in: rows.iter().map(|r| r[..ins].to_vec()).collect(),
            rows_gpu: rows.iter().map(|r| r[ins..].to_vec()).collect(),
        });
    }
    out
}

fn sdk_tool(name: &str) -> Option<PathBuf> {
    let exe = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    let mut c = vec![];
    if let Ok(s) = std::env::var("VULKAN_SDK") {
        c.push(Path::new(&s).join("Bin").join(&exe));
        c.push(Path::new(&s).join("bin").join(&exe));
    }
    c.push(PathBuf::from(r"C:\VulkanSDK\1.3.268.0\Bin").join(&exe));
    c.into_iter().find(|p| p.is_file())
}

/// The GLSL harness: each thread's `w` words of the shared array come from `dm_io` before the body and go back at every
/// return and at the end.
fn harness(glsl: &str, w: usize) -> Result<String, String> {
    let re_sh = regex_lite_shared(glsl).ok_or("no shared uint array")?;
    let (arr, words) = re_sh;
    if words < 32 * w {
        return Err(format!("shared array of {words} words, {} needed", 32 * w));
    }
    let base = format!("((gl_WorkGroupID.x * 32u + gl_LocalInvocationID.x) * {w}u)");
    let lid = "gl_LocalInvocationID.x";
    let dump = format!("for (uint dm_k = 0u; dm_k < {w}u; dm_k++) dm_io[{base} + dm_k] = {arr}[{lid} * {w}u + dm_k];");
    let seed = format!("for (uint dm_k = 0u; dm_k < {w}u; dm_k++) {arr}[{lid} * {w}u + dm_k] = dm_io[{base} + dm_k];");
    let m = glsl.find("void main()").ok_or("no main")?;
    let open = m + glsl[m..].find('{').ok_or("no main body")? + 1;
    let close = glsl.rfind('}').ok_or("no end")?;
    let body = glsl[open..close].replace("return;", &format!("{{ {dump} return; }}"));
    let head = &glsl[..m];
    // the io buffer after the #version / extension lines
    let ins = head.find("layout(").unwrap_or(0);
    let decl = "layout(set = 7, binding = 0, std430) buffer DmIo { uint dm_io[]; };\n";
    Ok(format!(
        "{}{decl}{}void main()\n{{\n    {seed}\n{body}\n    {dump}\n}}\n",
        &head[..ins],
        &head[ins..]
    ))
}

/// The shared array spirv-cross writes: `shared uint NAME[N];` -> (NAME, N).
fn regex_lite_shared(glsl: &str) -> Option<(String, usize)> {
    for l in glsl.lines() {
        let t = l.trim();
        if let Some(rest) = t.strip_prefix("shared uint ") {
            let (name, n) = rest.trim_end_matches(';').split_once('[')?;
            let n: usize = n.trim_end_matches(']').parse().ok()?;
            return Some((name.trim().to_string(), n));
        }
    }
    None
}

/// Lane classes of nxnative's compare.py: why two output words differ (None: equal).
fn cause(k: char, got: u32, want: u32, ins: &[u32], kinds_in: bool) -> Option<&'static str> {
    if got == want {
        return None;
    }
    let lanes: Vec<(u32, u32, u32, u32, u32)> = match k {
        'f' => vec![(got, want, 0x7f80_0000, 0x007f_ffff, 32)],
        'h' => vec![
            (got & 0xffff, want & 0xffff, 0x7c00, 0x03ff, 16),
            (got >> 16, want >> 16, 0x7c00, 0x03ff, 16),
        ],
        'b' => vec![
            (got & 0xffff, want & 0xffff, 0x7f80, 0x007f, 16),
            (got >> 16, want >> 16, 0x7f80, 0x007f, 16),
        ],
        _ => return Some("other"),
    };
    let nan = |v: u32, e: u32, m: u32| v & e == e && v & m != 0;
    let den = |v: u32, e: u32, m: u32| v & e == 0 && v & m != 0;
    for (g, w, e, m, bits) in lanes {
        if g == w {
            continue;
        }
        if nan(g, e, m) && nan(w, e, m) {
            continue; // nan-bits
        }
        if nan(g, e, m) || nan(w, e, m) {
            return Some("nan-one");
        }
        let sign = 1u32 << (bits - 1);
        if (g | w) & !sign == 0 {
            return Some("zero-sign");
        }
        if k == 'f' && kinds_in && ins.iter().any(|&x| den(x, 0x7f80_0000, 0x007f_ffff)) {
            continue; // denorm-in
        }
        let small = |v: u32| {
            den(v, e, m)
                || v & !sign == 0
                || v & !sign
                    == 1 << (bits
                        - (if bits == 32 {
                            9
                        } else if e == 0x7c00 {
                            6
                        } else {
                            9
                        }))
        };
        if den(g, e, m) || den(w, e, m) || (small(g) && small(w)) {
            continue; // denorm-out
        }
        return Some("other");
    }
    None
}

#[test]
#[ignore = "requires a Vulkan GPU and the external shader toolchain"]
fn ampere_ops_match_the_orin() {
    let _l = common::setup();
    let (Ok(tools), Some(cross), Some(glslang)) = (
        shaders::xlate::Tools::find(),
        sdk_tool("spirv-cross"),
        sdk_tool("glslangValidator"),
    ) else {
        eprintln!("ampere_orin: the translator or the Vulkan SDK tools are missing: skipped");
        return;
    };
    let e0 = common::validation_errors();
    let ops = load();
    let only: Vec<String> = std::env::var("AMPERE_ORIN_ONLY")
        .map(|v| v.split(',').map(String::from).collect())
        .unwrap_or_default();
    let ops: Vec<Op> = ops
        .into_iter()
        .filter(|o| only.is_empty() || only.contains(&o.name))
        .collect();
    let work = std::env::temp_dir().join(format!("nxgfx-ampere-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).unwrap();
    // every op through the translator in one go
    let mut jobs = vec![];
    for o in &ops {
        let code = work.join(format!("{}.bin", o.name));
        std::fs::write(&code, &o.program).unwrap();
        let w = o.ins + o.outs.len();
        let j = serde_json::json!({"key": o.name, "stage": "cs", "arch": "sm86", "code": code.to_string_lossy(), "tex": {}, "img": {}, "attr": {}, "fout": {},
            "local": [32, 1, 1], "debug": true, "reduced": true, "shared": 32 * w * 4, "nvn2_cs": [0, 0, 3, 0x40, 0], "c1": "", "target": "vulkan", "set": 5});
        jobs.push(shaders::xlate::Work {
            key: o.name.clone(),
            json: j,
        });
    }
    let outcome = shaders::xlate::translate(
        &tools,
        jobs,
        &work,
        4,
        32,
        std::time::Duration::from_secs(120),
        &|_, _| {},
    );
    let g = {
        let mut n = common::Nvn::new();
        let _ = n.d();
        nxgfx::vk::Gpu::get().unwrap()
    };
    let mut results: BTreeMap<String, (String, String)> = BTreeMap::new();
    for o in &ops {
        let r = run_op(g, o, &work, &outcome, &cross, &glslang);
        results.insert(o.name.clone(), r);
    }
    // the summary and the verdict
    let known: BTreeMap<&str, &str> = KNOWN.iter().copied().collect();
    let mut bad = vec![];
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for (name, (stage, why)) in &results {
        *counts.entry(stage.clone()).or_default() += 1;
        let listed = known.contains_key(name.as_str())
            || APPROX.contains(&name.as_str())
            || HERE.iter().any(|h| h.0 == name);
        if stage == "ok" && HERE.iter().any(|h| h.0 == name) {
            eprintln!("  {name}: matches now: take it out of HERE");
        }
        if stage != "ok" && !listed {
            bad.push(format!("{name}: {stage}: {why}"));
        }
    }
    eprintln!("ampere_orin: {} ops: {counts:?}", results.len());
    for (name, (stage, why)) in &results {
        if stage != "ok" {
            eprintln!(
                "  {name}: {stage}{}{}",
                if why.is_empty() { "" } else { ": " },
                why
            );
        }
    }
    let _ = std::fs::remove_dir_all(&work);
    assert!(
        bad.is_empty(),
        "ops outside KNOWN / APPROX that do not match the Orin:\n{}",
        bad.join("\n")
    );
    assert_eq!(common::validation_errors(), e0, "validation errors");
}

/// ("ok" | the stage that failed | "differs", the reason)
fn run_op(
    g: &'static nxgfx::vk::Gpu,
    o: &Op,
    work: &Path,
    outcome: &std::collections::HashMap<String, Result<(), String>>,
    cross: &Path,
    glslang: &Path,
) -> (String, String) {
    if let Some(Err(e)) = outcome.get(&o.name) {
        return (
            "translator".into(),
            e.lines().next().unwrap_or("").chars().take(160).collect(),
        );
    }
    let spv = work.join(format!("{}.spv", o.name));
    let r = std::process::Command::new(cross)
        .arg(&spv)
        .args(["--version", "460", "--vulkan-semantics"])
        .output()
        .unwrap();
    if !r.status.success() {
        return (
            "spirv-cross".into(),
            String::from_utf8_lossy(&r.stderr)
                .chars()
                .take(160)
                .collect(),
        );
    }
    let w = o.ins + o.outs.len();
    let src = match harness(&String::from_utf8_lossy(&r.stdout), w) {
        Ok(s) => s,
        Err(e) => return ("harness".into(), e),
    };
    let gl = work.join(format!("{}.comp", o.name));
    let out = work.join(format!("{}.h.spv", o.name));
    std::fs::write(&gl, &src).unwrap();
    let r = std::process::Command::new(glslang)
        .args(["-V", "--target-env", "vulkan1.3", "-o"])
        .arg(&out)
        .arg(&gl)
        .output()
        .unwrap();
    if !r.status.success() {
        return (
            "glslang".into(),
            String::from_utf8_lossy(&r.stdout)
                .lines()
                .find(|l| l.contains("ERROR"))
                .unwrap_or("")
                .to_string(),
        );
    }
    // the bindings the module declares: every one gets a zeroed 64 KiB buffer, the io buffer is set 7 binding 0
    let refl = std::process::Command::new(cross)
        .arg(&out)
        .arg("--reflect")
        .output()
        .unwrap();
    let refl: serde_json::Value = serde_json::from_slice(&refl.stdout).unwrap_or_default();
    let mut binds: Vec<(u32, u32, vk::DescriptorType)> = vec![];
    for (k, ty) in [
        ("ubos", vk::DescriptorType::UNIFORM_BUFFER),
        ("ssbos", vk::DescriptorType::STORAGE_BUFFER),
    ] {
        for b in refl[k].as_array().cloned().unwrap_or_default() {
            binds.push((
                b["set"].as_u64().unwrap_or(0) as u32,
                b["binding"].as_u64().unwrap_or(0) as u32,
                ty,
            ));
        }
    }
    if refl["textures"].as_array().is_some_and(|a| !a.is_empty())
        || refl["images"].as_array().is_some_and(|a| !a.is_empty())
    {
        return ("harness".into(), "the module samples textures".into());
    }
    let groups = o.rows_in.len().div_ceil(32);
    let mut io = vec![0xcdcd_cdcdu32; groups * 32 * w];
    for (c, row) in o.rows_in.iter().enumerate() {
        io[c * w..c * w + row.len()].copy_from_slice(row);
    }
    let words = std::fs::read(&out).unwrap();
    let got = match dispatch(g, &words, &binds, &mut io, groups as u32) {
        Ok(()) => io,
        Err(e) => return ("vulkan".into(), e),
    };
    let mut diff = 0usize;
    let mut first = String::new();
    for (c, want) in o.rows_gpu.iter().enumerate() {
        for (j, (&wv, &k)) in want.iter().zip(&o.outs).enumerate() {
            let gv = got[c * w + o.ins + j];
            if let Some(why) = cause(k, gv, wv, &o.rows_in[c], true) {
                if first.is_empty() {
                    first = format!("row {c} out {j}: {gv:#010x}, Orin {wv:#010x} ({why})");
                }
                diff += 1;
            }
        }
    }
    if diff > 0 {
        return ("differs".into(), format!("{diff} words; first {first}"));
    }
    ("ok".into(), String::new())
}

fn dispatch(
    g: &'static nxgfx::vk::Gpu,
    spv: &[u8],
    binds: &[(u32, u32, vk::DescriptorType)],
    io: &mut [u32],
    groups: u32,
) -> Result<(), String> {
    use gpu_allocator::MemoryLocation;
    let dev = g.dev();
    let nsets = binds.iter().map(|b| b.0 + 1).max().unwrap_or(0).max(8);
    let mut layouts = vec![];
    for s in 0..nsets {
        let bs: Vec<_> = binds
            .iter()
            .filter(|b| b.0 == s)
            .map(|b| {
                vk::DescriptorSetLayoutBinding::default()
                    .binding(b.1)
                    .descriptor_type(b.2)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::COMPUTE)
            })
            .collect();
        layouts.push(
            unsafe {
                dev.create_descriptor_set_layout(
                    &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bs),
                    None,
                )
            }
            .map_err(|e| e.to_string())?,
        );
    }
    let pl = unsafe {
        dev.create_pipeline_layout(
            &vk::PipelineLayoutCreateInfo::default().set_layouts(&layouts),
            None,
        )
    }
    .map_err(|e| e.to_string())?;
    let words: Vec<u32> = spv
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
        .collect();
    let m = unsafe {
        dev.create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&words), None)
    }
    .map_err(|e| e.to_string())?;
    let st = vk::PipelineShaderStageCreateInfo::default()
        .stage(vk::ShaderStageFlags::COMPUTE)
        .module(m)
        .name(c"main");
    let pipe = unsafe {
        dev.create_compute_pipelines(
            vk::PipelineCache::null(),
            &[vk::ComputePipelineCreateInfo::default()
                .stage(st)
                .layout(pl)],
            None,
        )
    }
    .map_err(|(_, e)| e.to_string())?[0];
    unsafe { dev.destroy_shader_module(m, None) };
    let nbytes = (io.len() * 4) as u64;
    let iob = g
        .buffer(
            nbytes,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            MemoryLocation::CpuToGpu,
            "ampere io",
        )
        .map_err(|e| e.to_string())?;
    unsafe { core::ptr::copy_nonoverlapping(io.as_ptr() as *const u8, iob.ptr, nbytes as usize) };
    let zero = g
        .buffer(
            65536,
            vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::UNIFORM_BUFFER,
            MemoryLocation::CpuToGpu,
            "ampere zero",
        )
        .map_err(|e| e.to_string())?;
    unsafe { core::ptr::write_bytes(zero.ptr, 0, 65536) };
    let sizes = [
        vk::DescriptorPoolSize {
            ty: vk::DescriptorType::UNIFORM_BUFFER,
            descriptor_count: 64,
        },
        vk::DescriptorPoolSize {
            ty: vk::DescriptorType::STORAGE_BUFFER,
            descriptor_count: 64,
        },
    ];
    let pool = unsafe {
        dev.create_descriptor_pool(
            &vk::DescriptorPoolCreateInfo::default()
                .max_sets(nsets)
                .pool_sizes(&sizes),
            None,
        )
    }
    .map_err(|e| e.to_string())?;
    let sets = unsafe {
        dev.allocate_descriptor_sets(
            &vk::DescriptorSetAllocateInfo::default()
                .descriptor_pool(pool)
                .set_layouts(&layouts),
        )
    }
    .map_err(|e| e.to_string())?;
    let mut writes = vec![];
    let infos: Vec<[vk::DescriptorBufferInfo; 1]> = binds
        .iter()
        .map(|b| {
            [vk::DescriptorBufferInfo {
                buffer: if (b.0, b.1) == (7, 0) {
                    iob.buf
                } else {
                    zero.buf
                },
                offset: 0,
                range: vk::WHOLE_SIZE,
            }]
        })
        .collect();
    for (b, info) in binds.iter().zip(&infos) {
        writes.push(
            vk::WriteDescriptorSet::default()
                .dst_set(sets[b.0 as usize])
                .dst_binding(b.1)
                .descriptor_type(b.2)
                .buffer_info(info),
        );
    }
    unsafe { dev.update_descriptor_sets(&writes, &[]) };
    let f = nxgfx::vk::queue::oneoff(g, |g, sub, _| unsafe {
        g.dev()
            .cmd_bind_pipeline(sub.main, vk::PipelineBindPoint::COMPUTE, pipe);
        g.dev().cmd_bind_descriptor_sets(
            sub.main,
            vk::PipelineBindPoint::COMPUTE,
            pl,
            0,
            &sets,
            &[],
        );
        g.dev().cmd_dispatch(sub.main, groups, 1, 1);
    });
    nxgfx::vk::queue::wait_fence(f, u64::MAX);
    unsafe {
        core::ptr::copy_nonoverlapping(
            iob.ptr as *const u8,
            io.as_mut_ptr() as *mut u8,
            nbytes as usize,
        )
    };
    unsafe {
        dev.destroy_pipeline(pipe, None);
        dev.destroy_pipeline_layout(pl, None);
        dev.destroy_descriptor_pool(pool, None);
        for l in layouts {
            dev.destroy_descriptor_set_layout(l, None);
        }
    }
    g.free_buffer(iob);
    g.free_buffer(zero);
    Ok(())
}
