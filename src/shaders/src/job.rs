//! A shader → the translator's job: the instructions it decodes (Maxwell after the 0x30 header, Ampere after the cut
//! to the instruction length), the c1 segment (indirect jump tables), and the facts the code cannot give (compute
//! thread groups, NVN2 compute's c0 layout and stage banks, task / mesh stage memory, early fragment tests).

use crate::nvn::*;
use serde_json::{json, Map, Value};

/// NVN2 graphics stages read their pushed buffer entries from bank 25 + stage index (vertex 0, hull / mesh 1,
/// domain 2, geometry 3, pixel 4).
pub const NVN2_STAGE_BANK: u32 = 25;

/// The descriptor set a stage's resources go to: the NVN stage number (vertex 0, fragment 1, geometry 2, tessellation
/// control 3, tessellation evaluation 4, compute 5), as the graphics module numbers stages; NVN2's task and mesh
/// stages 6 and 7. Each stage's resources start at binding 1; set 0 binding 0 is the translator's support buffer,
/// which every stage of a pipeline shares.
pub fn stage_set(st: Stage) -> u32 {
    match st {
        Stage::Vs => 0,
        Stage::Ps => 1,
        Stage::Gs => 2,
        Stage::Hs => 3,
        Stage::Ds => 4,
        Stage::Cs | Stage::Rt => 5,
        Stage::Ts => 6,
        Stage::Ms => 7,
    }
}

/// Everything the translation of one shader needs, and what the manifest records about it.
pub struct Job {
    pub key: String,
    pub stage: Stage,
    /// The stage the translator is told (task and mesh shaders are translated as compute).
    pub xlate_stage: Stage,
    pub code: Vec<u8>,
    pub json: Map<String, Value>,
    /// Facts for the graphics module that are not in the SPIR-V (tessellation mode, transform feedback, mesh info).
    pub extra: Map<String, Value>,
}

fn sph_outputs(sph: &[u8]) -> Vec<(Option<u32>, u32)> {
    let b = |i: usize| *sph.get(i).unwrap_or(&0) as u32;
    let mut out = vec![];
    if b(0x35) >> 3 & 1 != 0 {
        out.push((Some(u32::MAX), 0)); // the point size
    }
    out.extend(
        (0..4)
            .filter(|c| b(0x35) >> (4 + c) & 1 != 0)
            .map(|c| (None, c)),
    );
    for n in 0..32u32 {
        out.extend(
            (0..4)
                .filter(|c| b(0x36 + n as usize / 2) >> (4 * (n % 2) + c) & 1 != 0)
                .map(|c| (Some(n), c)),
        );
    }
    out
}

fn sph_system(sph: &[u8]) -> Vec<u32> {
    let bits =
        *sph.get(0x34).unwrap_or(&0) as u32 | ((*sph.get(0x35).unwrap_or(&0) as u32) & 0xf) << 8;
    (0..12).filter(|b| bits >> b & 1 != 0).collect()
}

/// A task or mesh shader's facts and its stage memory layout (the translator keeps the stage memory in shared
/// memory): → (info, stage_memory offsets, shared bytes, threads).
fn mesh_stage(ctl: &[u8], area: &[u8]) -> (Value, Vec<u32>, u32, u32) {
    const WORKGROUP: u32 = 0;
    const INPUT: u32 = 0x10;
    const BLOCK: u32 = 32;
    let blocks_of = |n: u32| (n.max(1) + BLOCK - 1) / BLOCK;
    let kind = if ctl.get(0x56) == Some(&6) {
        "ts"
    } else {
        "ms"
    };
    let threads = u16_at(ctl, 0x58).unwrap_or(0) as u32;
    let payload = u16_at(ctl, 0x5a).unwrap_or(0) as u32;
    let out = INPUT + ((payload + 15) & !15);
    if kind == "ts" {
        let info = json!({"kind": kind, "threads": threads, "payload": payload, "memory": [WORKGROUP, INPUT, out, out + payload]});
        return (
            info,
            vec![WORKGROUP, INPUT, out, out + payload],
            out + ((payload + 3) & !3),
            threads,
        );
    }
    let (max_v, max_p) = (
        u16_at(ctl, 0x60).unwrap_or(0) as u32,
        u16_at(ctl, 0x62).unwrap_or(0) as u32,
    );
    let at = if ctl.get(0x68).is_some_and(|b| b & 0x80 != 0) {
        u32_at(ctl, 0x64).unwrap_or(0) as usize
    } else {
        0
    };
    let psph = if at != 0 {
        area.get(at..(at + 0x80).min(area.len())).unwrap_or(&[])
    } else {
        &[]
    };
    let vertex = sph_outputs(area.get(..0x80.min(area.len())).unwrap_or(&[]));
    let primitive: Vec<_> = if at != 0 {
        sph_outputs(psph)
            .into_iter()
            .filter(|c| c.0.is_some_and(|n| n != u32::MAX))
            .collect()
    } else {
        vec![]
    };
    let psys = if at != 0 { sph_system(psph) } else { vec![] };
    let verts = match ctl.get(0x5f) {
        Some(&0) | None => 3,
        Some(&v) => v as u32,
    };
    let (vb, pb) = (blocks_of(max_v), blocks_of(max_p));
    let per_prim = psys.len() as u32 + primitive.len() as u32;
    let prim = out + (vb * vertex.len() as u32 + pb * per_prim).max(1) * BLOCK * 4;
    let n = (vertex.len() as u32).max(1);
    let mem = vec![WORKGROUP, INPUT, out, prim, prim - out, n, max_v * 4 * n];
    let shared = prim + ((4 + verts * pb * BLOCK + 3) & !3);
    let info = json!({"kind": kind, "threads": threads, "payload": payload, "task": ctl.get(0x5e).is_some_and(|&b| b != 0), "verts": verts,
        "max_v": max_v, "max_p": max_p, "vertex_components": vertex.len(), "primitive_components": primitive.len(), "primitive_system": psys,
        "vertex": comp_list(&vertex), "primitive": comp_list(&primitive), "memory": mem.clone()});
    (info, mem, shared, threads)
}

/// An attribute area's components as `[attribute, component]`: -2 the point size, -1 the position, n generic n.
fn comp_list(c: &[(Option<u32>, u32)]) -> Value {
    json!(c
        .iter()
        .map(|&(a, k)| json!([
            match a {
                None => -1i64,
                Some(u32::MAX) => -2,
                Some(n) => n as i64,
            },
            k
        ]))
        .collect::<Vec<_>>())
}

/// Whether Ampere instructions use shared memory of their own (LDS, STS, ATOMS), which would lie over the stage memory.
fn own_shared(code: &[u8]) -> bool {
    (0..code.len().saturating_sub(15)).step_by(16).any(|o| {
        matches!(
            u16_at(code, o).unwrap() & 0xfff,
            0x984 | 0x388 | 0x988 | 0x38c | 0x98c | 0xf8c
        )
    })
}

/// A vertex shader in two parts (VertexA + VertexB, Catherine: Full Body's GLSLC) merged into one program, as
/// Ryujinx's Translate(VertexA) builds it: A's instructions with each unconditional EXIT a branch to B, then B's,
/// under B's SPH with A's maps ORed in. → (code area with its 0x30 header, c1), or None for any other vertex shader.
pub fn vertex_ab(key: &str, code: &[u8], ctl: &[u8]) -> R<Option<(Vec<u8>, Vec<u8>)>> {
    if code.len() < 0x80 || ctl.len() < 0x760 || code[..4] != GFX_MAGIC {
        return Ok(None);
    }
    let w0 = u32_at(code, 0x30).unwrap();
    if w0 & 0x1f != 1 || (w0 >> 10) & 0xf != 0 {
        return Ok(None);
    }
    let v: Vec<usize> = (0..6)
        .map(|i| u32_at(ctl, 0x748 + 4 * i).unwrap_or(0) as usize)
        .collect();
    let (a_sph, a_len, b_sph, b_len) = (v[0], v[1], v[3], v[4]);
    if a_sph == 0 && a_len == 0 && b_sph == 0 && b_len == 0 {
        return Ok(None);
    }
    let bw0 = if 0x80 <= b_sph && b_sph + 0x50 <= code.len() {
        u32_at(code, b_sph).unwrap()
    } else {
        0
    };
    let c1o = u32_at(ctl, 0x700).unwrap_or(0) as usize;
    if a_sph != 0x30
        || b_sph != c1o + 0x30
        || bw0 & 0x1f != 1
        || (bw0 >> 10) & 0xf != 1
        || a_len < 0x50
        || b_len < 0x50
        || (a_len - 0x50) % 32 != 0
        || (b_len - 0x50) % 32 != 0
        || b_sph + b_len > code.len()
        || a_sph + a_len > code.len()
    {
        return Err(format!("{key}: vertex shader of shader type 0 without a VertexB part in the expected place (control +0x748 {a_sph:#x} {a_len:#x} {b_sph:#x} {b_len:#x})"));
    }
    let sa: Vec<u32> = (0..20)
        .map(|i| u32_at(code, a_sph + 4 * i).unwrap())
        .collect();
    let sb: Vec<u32> = (0..20)
        .map(|i| u32_at(code, b_sph + 4 * i).unwrap())
        .collect();
    let mut sph = vec![sb[0] | (sa[0] & !(0xf << 10))];
    for i in 1..4 {
        sph.push((sa[i] & 0xffffff).max(sb[i] & 0xffffff) | (sb[i] & !0xffffff));
    }
    sph.push(sb[4]);
    for i in 5..20 {
        sph.push(sa[i] | sb[i]);
    }
    let mut a = code[a_sph + 0x50..a_sph + a_len].to_vec();
    let b = &code[b_sph + 0x50..b_sph + b_len];
    let alen = a.len();
    for o in (0..alen).step_by(8) {
        if o % 32 == 0 {
            continue; // the scheduling word of each group of three instructions
        }
        let ins = u64::from_le_bytes(a[o..o + 8].try_into().unwrap());
        if ins >> 52 == 0xe30 && ins & 0x1f == 0xf {
            let bra =
                0xe24u64 << 52 | (((alen - (o + 8)) as u64) & 0xffffff) << 20 | (ins & 0xfffff);
            a[o..o + 8].copy_from_slice(&bra.to_le_bytes());
        }
    }
    let mut out = code[..0x30].to_vec();
    for w in sph {
        out.extend_from_slice(&w.to_le_bytes());
    }
    out.extend_from_slice(&a);
    out.extend_from_slice(b);
    Ok(Some((out, code[b_sph + b_len..].to_vec())))
}

/// Transform feedback streams of an NVN control section: [{stride, outputs: [[attribute, components]]}] or None.
fn tf_layout(ctl: &[u8]) -> Option<Value> {
    let strides: Vec<u32> = (0..4)
        .map(|b| u16_at(ctl, 0x6dc + 2 * b).unwrap_or(0) as u32)
        .collect();
    if strides.iter().all(|&s| s == 0) {
        return None;
    }
    let mut out = vec![];
    for b in 0..4 {
        if strides[b] == 0 {
            out.push(json!([]));
            continue;
        }
        let count = *ctl.get(0x6e8 + b)? as usize;
        let locs = ctl.get(0xdc + 0x80 * b..0xdc + 0x80 * b + count)?;
        let sizes = ctl.get(0x4dc + 0x80 * b..0x4dc + 0x80 * b + count)?;
        if !(1..=128).contains(&count)
            || locs.iter().any(|&x| !(0x20..0x40).contains(&x))
            || sizes.iter().any(|&n| !(1..=4).contains(&n))
            || strides[b] != 4 * sizes.iter().map(|&n| n as u32).sum::<u32>()
        {
            return None;
        }
        out.push(json!({"stride": strides[b], "outputs": locs.iter().zip(sizes).map(|(&o, &n)| json!([o - 0x20, n])).collect::<Vec<_>>()}));
    }
    Some(Value::Array(out))
}

const TESS_MODES: [(u32, &str); 3] = [
    (0x0004, "triangles"),
    (0x0007, "quads"),
    (0x8E7A, "isolines"),
];
const TESS_SPACING: [(u32, &str); 3] = [
    (0x0202, "equal"),
    (0x8E7B, "fractional_odd"),
    (0x8E7C, "fractional_even"),
];
const TESS_ORDER: [(u32, &str); 2] = [(0x0900, "cw"), (0x0901, "ccw")];

/// A domain shader's own tessellation mode from its NVN control section.
fn tess_mode(ctl: &[u8]) -> Option<Value> {
    if ctl.len() < 0x748 {
        return None;
    }
    let find = |t: &[(u32, &'static str)], v: u32| t.iter().find(|x| x.0 == v).map(|x| x.1);
    Some(
        json!({"mode": find(&TESS_MODES, u32_at(ctl, 0x738)?)?, "spacing": find(&TESS_SPACING, u32_at(ctl, 0x73c)?)?, "winding": find(&TESS_ORDER, u32_at(ctl, 0x744)?)?}),
    )
}

/// Translation options: `debug` keeps the translator's DebugMode (no AST optimizer, as nxnative ships it);
/// `reduced` lets it contract float operations that need not be exact.
#[derive(Clone, Copy)]
pub struct JobOptions {
    pub debug: bool,
    pub reduced: bool,
}

impl Default for JobOptions {
    fn default() -> Self {
        JobOptions {
            debug: true,
            reduced: true,
        }
    }
}

/// An Ampere SPH (not an instruction) at the start of a code area. Word 0 as every NVN2 graphics SPH of the verification
/// games has it: SPH type 1 or 2 (bits 0..4), version 4 (bits 5..9), shader type 1..5 (bits 10..13), byte 2 0x0c or
/// 0x0d; and bytes 13..16 zero, where every instruction has its scheduling controls (stall count, barriers, yield).
pub fn is_ampere_sph(code: &[u8]) -> bool {
    let Some(w) = u32_at(code, 0) else {
        return false;
    };
    let ty = (w >> 10) & 0xf;
    matches!(w & 0x1f, 1 | 2)
        && (w >> 5) & 0x1f == 4
        && (1..=5).contains(&ty)
        && matches!((w >> 16) & 0xff, 0x0c | 0x0d)
        && code.get(13..16).is_some_and(|b| b == [0, 0, 0])
        && code.len() > 0x80
}

#[cfg(test)]
mod sph_tests {
    #[test]
    fn sph_or_instruction() {
        let mut sph = vec![0u8; 0x90];
        sph[..4].copy_from_slice(&0x020c0481u32.to_le_bytes());
        assert!(super::is_ampere_sph(&sph));
        // S2R R2 (MKW compute 0028b5da242b07b2) and an FADD with predicate P0
        let s2r = [
            0x19u8, 0x79, 2, 0, 0, 0, 0, 0, 0, 0x25, 0, 0, 0, 0x28, 0x0e, 0,
        ];
        assert!(!super::is_ampere_sph(&[&s2r[..], &[0u8; 0x80]].concat()));
        let mut fadd = vec![0u8; 0x90];
        fadd[..4].copy_from_slice(&0x020c0421u32.to_le_bytes());
        fadd[13] = 0xe2;
        assert!(!super::is_ampere_sph(&fadd));
    }
}

pub fn job_for(key: &str, stage: Stage, ctl: &[u8], area: &[u8], opts: JobOptions) -> R<Job> {
    let mut j = Map::new();
    let mut extra = Map::new();
    let mut st = stage;
    let code: Vec<u8>;
    let c1: Vec<u8>;
    if stage == Stage::Rt {
        return Err(format!("{key}: ray-tracing programs are not translated (they need the RT program description nxnative builds)"));
    }
    if nvn2_ctl(ctl) {
        let (a, e) = (
            u32_at(ctl, 0x40).unwrap_or(0) as usize,
            u32_at(ctl, 0x44).unwrap_or(0) as usize,
        );
        c1 = if a <= e && e <= area.len() {
            area[a..e].to_vec()
        } else {
            vec![]
        };
        let il = (u32_at(ctl, 0x38).unwrap_or(0) as usize).min(area.len());
        let mut cut = area[..il].to_vec();
        j.insert("arch".into(), json!("sm86"));
        if stage == Stage::Cs {
            let local: Vec<u32> = (0..3)
                .map(|i| u32_at(ctl, 0x58 + 4 * i).unwrap_or(0))
                .collect();
            if local.iter().any(|&x| x == 0) {
                return Err(format!("{key}: zero compute workgroup dimension"));
            }
            j.insert("local".into(), json!(local));
            j.insert(
                "shared".into(),
                json!((u32_at(ctl, 0x64).unwrap_or(0) + 15) / 16 * 16),
            );
            let b = |o: usize| *ctl.get(o).unwrap_or(&0) as u32;
            j.insert(
                "nvn2_cs".into(),
                json!([
                    b(0xb1),
                    b(0xb2),
                    b(0xb3),
                    u16_at(ctl, 0xba).unwrap_or(0),
                    b(0xa9) * 4,
                    b(0xb4),
                    b(0xb5),
                    b(0xaa) * 4
                ]),
            );
            // A compute program whose code area starts with a 0x80-byte SPH (Mario Kart World's DrivePassShaderDefault
            // 078997bf631992c5: word 0 0x020c0481, SPH type 1, then S2R / I2F / ... EXIT / BRA-to-self from +0x80):
            // the instructions start after it. Ampere's opcode 0x481 does not exist, so such a word is never an
            // instruction.
            if cut.len() > 0x80 && is_ampere_sph(&cut) {
                cut = cut[0x80..].to_vec();
                extra.insert(
                    "cs_sph".into(),
                    json!("a 0x80-byte SPH before the compute instructions, taken off"),
                );
            }
        }
        let idx = match stage {
            Stage::Vs => Some(0),
            Stage::Hs | Stage::Ms => Some(1),
            Stage::Ds => Some(2),
            Stage::Gs => Some(3),
            Stage::Ps => Some(4),
            _ => None,
        };
        if let Some(i) = idx {
            j.insert("stage_bank".into(), json!(NVN2_STAGE_BANK + i));
        }
        if matches!(stage, Stage::Ts | Stage::Ms) {
            let (info, mem, shared, threads) = mesh_stage(ctl, area);
            if cut.len() < 0x80 {
                return Err(format!("{key}: a {stage} shader shorter than its SPH"));
            }
            cut = cut[0x80..].to_vec();
            if own_shared(&cut) {
                extra.insert("mesh_own_shared".into(), json!(true));
            }
            extra.insert("mesh_stage".into(), info);
            j.insert("stage_memory".into(), json!(mem));
            j.insert("local".into(), json!([threads.max(1), 1, 1]));
            j.insert("shared".into(), json!(shared));
            st = Stage::Cs;
        }
        code = cut;
        if stage == Stage::Ps
            && ctl.get(0x5b).is_some_and(|&b| b != 0)
            && code.len() >= 0x50
            && u32_at(&code, 19 * 4).unwrap() & 2 == 0
        {
            j.insert("early_fragment_tests".into(), json!(true));
        }
    } else {
        let (c1o, c1t) = (
            u32_at(ctl, 0x700).unwrap_or(0) as usize,
            u32_at(ctl, 0x704).unwrap_or(0) as usize,
        );
        let mut c1v = if c1o <= c1t && c1t <= area.len() {
            area[c1o..c1t].to_vec()
        } else {
            vec![]
        };
        let mut k = area.to_vec();
        if stage == Stage::Vs {
            if let Some((c, c1b)) = vertex_ab(key, area, ctl)? {
                k = c;
                c1v = c1b;
                extra.insert("vertex_ab".into(), json!(true));
            }
        }
        if stage != Stage::Cs {
            if k.len() < 0x30 {
                return Err(format!("{key}: a code area shorter than its header"));
            }
            k = k[0x30..].to_vec();
        } else {
            if k.get(..4) == Some(&CS_MAGIC[..]) {
                if k.len() < 0x100 || k[0x30..0x100].iter().any(|&b| b != 0) {
                    return Err(format!("{key}: compute code area +0x30..+0x100 is not all zero, so the instructions do not start at +0x100"));
                }
                k = k[0x100..].to_vec();
            }
            let local: Vec<u32> = (0..3)
                .map(|i| u32_at(ctl, 0x748 + 4 * i).unwrap_or(0))
                .collect();
            if local.iter().any(|&x| x == 0) {
                return Err(format!("{key}: compute control +0x748 thread group is 0"));
            }
            j.insert("local".into(), json!(local));
            j.insert(
                "shared".into(),
                json!((u32_at(ctl, 0x754).unwrap_or(0) + 15) / 16 * 16),
            );
        }
        if matches!(stage, Stage::Vs | Stage::Gs) {
            if let Some(tf) = tf_layout(ctl) {
                extra.insert("transform_feedback".into(), tf);
            }
        }
        if stage == Stage::Ds {
            if let Some(t) = tess_mode(ctl) {
                extra.insert("tess_mode".into(), t);
            }
        }
        if stage == Stage::Ps
            && ctl.get(0x718).is_some_and(|&b| b != 0)
            && k.len() >= 0x50
            && u32_at(&k, 19 * 4).unwrap() & 2 == 0
        {
            j.insert("early_fragment_tests".into(), json!(true));
        }
        c1 = c1v;
        code = k;
    }
    j.insert("key".into(), json!(key));
    j.insert("stage".into(), json!(st.name()));
    j.insert(
        "c1".into(),
        json!(c1.iter().map(|b| format!("{b:02x}")).collect::<String>()),
    );
    j.insert("tex".into(), json!({}));
    j.insert("img".into(), json!({}));
    j.insert("attr".into(), json!({}));
    j.insert("fout".into(), json!({}));
    j.insert("debug".into(), json!(opts.debug));
    j.insert("reduced".into(), json!(opts.reduced));
    j.insert("target".into(), json!("vulkan"));
    j.insert("set".into(), json!(stage_set(stage)));
    if j.contains_key("early_fragment_tests") {
        extra.insert("early_fragment_tests".into(), json!(true));
    }
    if let Some(l) = j.get("local") {
        extra.insert("local".into(), l.clone());
    }
    Ok(Job {
        key: key.into(),
        stage,
        xlate_stage: st,
        code,
        json: j,
        extra,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nvn::synth::*;

    #[test]
    fn nvn1_and_nvn2_jobs() {
        let (c, k) = nvn1_ps(1);
        let j = job_for("k1", Stage::Ps, &c, &k, JobOptions::default()).unwrap();
        assert_eq!(j.code.len(), k.len() - 0x30);
        assert_eq!(j.json["set"], json!(1));
        assert!(j.json.get("arch").is_none());
        let (c, k) = nvn2(Stage::Cs, 1);
        let j = job_for("k2", Stage::Cs, &c, &k, JobOptions::default()).unwrap();
        assert_eq!(j.json["arch"], json!("sm86"));
        assert_eq!(j.json["local"], json!([32, 1, 1]));
        assert_eq!(j.code.len(), 0xc0);
        // c1 lies between +0x40 and +0x44 of the code area
        assert_eq!(j.json["c1"].as_str().unwrap().len(), 2 * (k.len() - 0x100));
        let (c, k) = nvn2(Stage::Vs, 1);
        let j = job_for("k3", Stage::Vs, &c, &k, JobOptions::default()).unwrap();
        assert_eq!(j.json["stage_bank"], json!(25));
    }
}
