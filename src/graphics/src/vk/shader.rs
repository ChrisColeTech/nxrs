//! The shader library: one translated SPIR-V module per shader stage, found by the stage's shader key
//! (`nvn::prog`), in lane C's layout (docs/INTERFACES.md "shaders"):
//!
//! - `<dir>/spv/<key>.spv` and `<dir>/spv/<key>.json` (`<dir>` = the game's `<data>/shaders`; `<dir>/<key>.*` is read
//!   too), the json's `status` "ok";
//! - the json's `bindings[]` say what the game binds at each (set, binding) the translator chose. At load each one is
//!   moved into the shared layout (`layout`) by rewriting the module's DescriptorSet / Binding decorations, and the
//!   module keeps the list of what to bind there (`Res`).
//!
//! `insert` adds modules in memory (tests, tools, a run-time translator).

use super::layout as L;
use crate::nvn::prog::{key_hex, ShaderKey};
use ash::vk;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// The device's maxCombinedClipAndCullDistances (0 until the device is made): modules declaring more are trimmed.
/// `NXGFX_LOOP_GUARD`: the back-edges a shader invocation's function may take (0: loops left unbounded).
pub fn loop_cap() -> u32 {
    static C: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *C.get_or_init(|| {
        std::env::var("NXGFX_LOOP_GUARD")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(65536)
    })
}

pub static CLIP_CULL_LIMIT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InKind {
    Float,
    Sint,
    Uint,
}

/// Numeric class delivered by Vulkan vertex fetch (normalized/scaled formats deliver floats).
pub fn vertex_fetch_kind(f: vk::Format) -> Option<InKind> {
    use vk::Format as F;
    Some(match f {
        F::UNDEFINED => return None,
        F::R8_UINT
        | F::R8G8_UINT
        | F::R8G8B8_UINT
        | F::R8G8B8A8_UINT
        | F::B8G8R8_UINT
        | F::B8G8R8A8_UINT
        | F::A8B8G8R8_UINT_PACK32
        | F::R16_UINT
        | F::R16G16_UINT
        | F::R16G16B16_UINT
        | F::R16G16B16A16_UINT
        | F::R32_UINT
        | F::R32G32_UINT
        | F::R32G32B32_UINT
        | F::R32G32B32A32_UINT
        | F::A2B10G10R10_UINT_PACK32
        | F::A2R10G10B10_UINT_PACK32 => InKind::Uint,
        F::R8_SINT
        | F::R8G8_SINT
        | F::R8G8B8_SINT
        | F::R8G8B8A8_SINT
        | F::B8G8R8_SINT
        | F::B8G8R8A8_SINT
        | F::A8B8G8R8_SINT_PACK32
        | F::R16_SINT
        | F::R16G16_SINT
        | F::R16G16B16_SINT
        | F::R16G16B16A16_SINT
        | F::R32_SINT
        | F::R32G32_SINT
        | F::R32G32B32_SINT
        | F::R32G32B32A32_SINT
        | F::A2B10G10R10_SINT_PACK32
        | F::A2R10G10B10_SINT_PACK32 => InKind::Sint,
        _ => InKind::Float,
    })
}

/// NVN registers carry bits regardless of the attribute's numeric class. Vulkan instead requires the input
/// declaration to match the fetch format. Retype scalar/vec2..4 32-bit inputs and bitcast each load back to its
/// original type, preserving the translated body's register semantics (not a numeric int-to-float conversion).
/// Unsupported pointer uses are refused rather than producing a partially retyped module.
fn retype_vertex_inputs(w: &[u32], wanted: &[(u32, InKind)]) -> Option<Vec<u32>> {
    type Types = HashMap<u32, (u32, Vec<u32>)>;
    fn shape(t: &Types, id: u32) -> Option<(InKind, u32)> {
        let (op, a) = t.get(&id)?;
        match (*op, a.as_slice()) {
            (21, [32, 0]) => Some((InKind::Uint, 1)),
            (21, [32, 1]) => Some((InKind::Sint, 1)),
            (22, [32]) => Some((InKind::Float, 1)),
            (23, [scalar, n]) if (2..=4).contains(n) => {
                let (k, lanes) = shape(t, *scalar)?;
                (lanes == 1).then_some((k, *n))
            }
            _ => None,
        }
    }
    fn intern(t: &mut Types, decl: &mut Vec<u32>, bound: &mut u32, op: u32, a: &[u32]) -> u32 {
        if let Some(id) = t
            .iter()
            .filter(|(_, (o, args))| *o == op && args == a)
            .map(|(&id, _)| id)
            .min()
        {
            return id;
        }
        let id = *bound;
        *bound += 1;
        decl.push(((a.len() as u32 + 2) << 16) | op);
        decl.push(id);
        decl.extend_from_slice(a);
        t.insert(id, (op, a.to_vec()));
        id
    }
    fn replacement(
        t: &mut Types,
        decl: &mut Vec<u32>,
        bound: &mut u32,
        old_ptr: u32,
        k: InKind,
    ) -> Option<(u32, u32)> {
        let (32, a) = t.get(&old_ptr)? else {
            return None;
        };
        if a.len() != 2 || a[0] != 1 {
            return None;
        }
        let (_, n) = shape(t, a[1])?;
        let scalar = match k {
            InKind::Float => intern(t, decl, bound, 22, &[32]),
            InKind::Uint => intern(t, decl, bound, 21, &[32, 0]),
            InKind::Sint => intern(t, decl, bound, 21, &[32, 1]),
        };
        let value = if n == 1 {
            scalar
        } else {
            intern(t, decl, bound, 23, &[scalar, n])
        };
        let ptr = intern(t, decl, bound, 32, &[1, value]);
        Some((ptr, value))
    }
    if w.len() < 5 || w[0] != 0x0723_0203 || wanted.is_empty() {
        return None;
    }
    let mut ops = Vec::new();
    let mut types = Types::new();
    let mut locations = HashMap::new();
    let mut at = 5;
    let mut vertex = false;
    while at < w.len() {
        let n = (w[at] >> 16) as usize;
        if n == 0 || at + n > w.len() {
            return None;
        }
        let op = w[at] & 0xffff;
        let a = &w[at + 1..at + n];
        match op {
            15 if a.len() >= 3 => {
                if a[0] != 0 {
                    return None;
                }
                vertex = true;
            }
            21..=32 if !a.is_empty() => {
                types.insert(a[0], (op, a[1..].to_vec()));
            }
            71 if a.len() >= 3 && a[1] == 30 => {
                locations.insert(a[0], a[2]);
            }
            _ => {}
        }
        ops.push((at, op, n));
        at += n;
    }
    if !vertex {
        return None;
    }
    let mut bound = w[3];
    let mut decl = Vec::new();
    let mut roots = HashMap::new();
    let mut kinds = HashMap::new();
    let mut repl = HashMap::new(); // pointer value -> (new pointer type, new loaded type)
    for &(i, op, n) in &ops {
        if op != 59 || n < 4 || w[i + 3] != 1 {
            continue;
        }
        let id = w[i + 2];
        let Some(&loc) = locations.get(&id) else {
            continue;
        };
        let Some(&(_, k)) = wanted.iter().find(|&&(l, _)| l == loc) else {
            continue;
        };
        let (32, p) = types.get(&w[i + 1])? else {
            return None;
        };
        if p.len() != 2 {
            return None;
        }
        if shape(&types, p[1])?.0 == k {
            continue;
        }
        if n != 4 {
            return None;
        }
        let pair = replacement(&mut types, &mut decl, &mut bound, w[i + 1], k)?;
        roots.insert(id, pair.0);
        kinds.insert(id, k);
        repl.insert(id, pair);
    }
    if roots.is_empty() {
        return None;
    }
    for &(i, op, n) in &ops {
        if matches!(op, 65 | 66) && n >= 4 {
            if let Some(&k) = kinds.get(&w[i + 3]) {
                // These inputs are scalars/vectors, not arrays or interface structs.
                if n != 5 {
                    return None;
                }
                let pair = replacement(&mut types, &mut decl, &mut bound, w[i + 1], k)?;
                kinds.insert(w[i + 2], k);
                repl.insert(w[i + 2], pair);
            }
        }
    }
    let mut out = w[..5].to_vec();
    let mut vars = Vec::new();
    let mut declared = false;
    let mut in_function = false;
    for &(i, op, n) in &ops {
        let a = &w[i + 1..i + n];
        if op == 54 {
            if !declared {
                // Types can be interleaved with globals in the input. Move only affected variables here so every
                // existing type is available and new types never duplicate a later existing declaration.
                out.extend_from_slice(&decl);
                out.extend_from_slice(&vars);
                declared = true;
            }
            in_function = true;
        }
        if op == 59 && n >= 4 && roots.contains_key(&a[1]) {
            vars.extend_from_slice(&[w[i], roots[&a[1]], a[1], 1]);
            continue;
        }
        if matches!(op, 65 | 66) && n >= 4 && repl.contains_key(&a[1]) {
            out.push(w[i]);
            out.push(repl[&a[1]].0);
            out.extend_from_slice(&a[1..]);
            continue;
        }
        if op == 61 && n >= 4 {
            if let Some(&(_, value)) = repl.get(&a[2]) {
                // Preserve result ID and body type; only the fetch itself has the format's class.
                shape(&types, a[0])?;
                let tmp = bound;
                bound += 1;
                out.extend_from_slice(&[w[i], value, tmp]);
                out.extend_from_slice(&a[2..]);
                out.extend_from_slice(&[(4 << 16) | 124, a[0], a[1], tmp]);
                continue;
            }
        }
        if in_function {
            let ids = match op {
                8 | 317 | 54 | 56 => &[][..],     // debug lines, function declarations
                12 => a.get(4..).unwrap_or(&[]),  // ExtInst number is a literal
                79 => a.get(2..4).unwrap_or(&[]), // vector shuffle selectors are literals
                81 => a.get(2..3).unwrap_or(&[]), // composite indices are literals
                82 => a.get(2..4).unwrap_or(&[]),
                246 => a.get(..2).unwrap_or(&[]),
                247 => a.get(..1).unwrap_or(&[]),
                250 => a.get(..3).unwrap_or(&[]),
                251 => a.get(..2).unwrap_or(&[]),
                _ => a,
            };
            if ids.iter().any(|id| repl.contains_key(id)) {
                return None;
            }
        }
        out.extend_from_slice(&w[i..i + n]);
        if op == 56 {
            in_function = false;
        }
    }
    if !declared {
        return None;
    }
    out[3] = bound;
    Some(out)
}

/// What the host binds at one binding of a module's stage set.
#[derive(Clone, Debug, PartialEq)]
pub enum Res {
    /// constant bank c[bank]
    Cbuf {
        bank: u32,
    },
    Support,
    /// a storage buffer whose {u64 address, u32 size} is c[cb_slot] at byte `cb_off`
    Sbuf {
        cb_slot: i32,
        cb_off: u32,
        written: bool,
    },
    /// a texture whose handle word(s) are c[cbuf_slot] (-1 = c2) at word `handle & 0x3fff` (sampler word at
    /// `handle >> 14 & 0x3fff` for a separate sampler handle); `ty` is the sampler type (dimension | array 0x100)
    Tex {
        handle: u32,
        cbuf_slot: i32,
        ty: u32,
    },
    /// `fmt`: the format the shader declares for the image (an NVN format code; 0: none, the texture's own): a raw
    /// surface load (SULD.B, `r32ui`) reads the 32-bit words of an RGBA8 image, so its view is that format
    Img {
        handle: u32,
        cbuf_slot: i32,
        ty: u32,
        fmt: u16,
    },
    /// NVN's texture pool / sampler pool / image pool indexed by ID (the pool set)
    Pool,
    /// a task / mesh shader's draw records (`mesh_draw`: {count, first} per draw; lane S)
    MeshDraw,
}

pub struct Module {
    pub key: ShaderKey,
    /// NVN stage (0 vertex .. 5 compute)
    pub stage: u32,
    pub module: vk::ShaderModule,
    pub entry: std::ffi::CString,
    /// (binding in the module's stage set, what goes there)
    pub res: Vec<(u32, Res)>,
    pub uses_pool: bool,
    /// vertex stage inputs: (location, kind)
    pub inputs: Vec<(u32, InKind)>,
    pub c1: Option<Vec<u8>>,
    pub id: u64,
    /// a pixel shader's output map (SPH OmapTargets: 4 bits per colour target, R G B A from the low bit): the GPU
    /// writes only these components of a bound target and leaves the others as they were
    pub omap: Option<u32>,
    /// 1 a task shader, 2 a mesh shader (in the tessellation stages' places 3 / 4)
    pub mesh: u8,
    /// what the program captures with transform feedback (the reflection's `transform_feedback`)
    pub xfb: Option<super::xfb::Layout>,
    /// the module's words as bound (its transform feedback variant is made from them)
    pub words: Vec<u32>,
    /// a mesh shader's per-primitive output locations (the pixel shader drawn with it reads them per primitive)
    pub per_primitive: Vec<u32>,
}

impl Module {
    /// The Vulkan stage the module runs as.
    pub fn vk_stage(&self) -> vk::ShaderStageFlags {
        match self.mesh {
            1 => vk::ShaderStageFlags::TASK_EXT,
            2 => vk::ShaderStageFlags::MESH_EXT,
            _ => super::layout::stage_flag(self.stage),
        }
    }
}

pub struct Refl {
    pub stage: u32,
    /// 1 a task shader, 2 a mesh shader (stage 3 / 4)
    pub mesh: u8,
    pub entry: String,
    /// (variable id, set, binding)
    pub vars: Vec<(u32, u32, u32)>,
    pub inputs: Vec<(u32, InKind)>,
}

/// The parts of a SPIR-V module the replay needs.
pub fn reflect(words: &[u32]) -> Result<Refl, String> {
    if words.len() < 5 || words[0] != 0x0723_0203 {
        return Err("not SPIR-V".into());
    }
    let mut i = 5;
    let mut stage = None;
    let mut mesh = 0u8;
    let mut entry = String::from("main");
    let mut set: HashMap<u32, u32> = HashMap::new();
    let mut binding: HashMap<u32, u32> = HashMap::new();
    let mut location: HashMap<u32, u32> = HashMap::new();
    let mut builtin: HashMap<u32, bool> = HashMap::new();
    let mut vars: Vec<(u32, u32, u32)> = Vec::new();
    let mut ptr_types: HashMap<u32, (u32, u32)> = HashMap::new();
    let mut types: HashMap<u32, (u32, Vec<u32>)> = HashMap::new();
    while i < words.len() {
        let w = words[i];
        let n = (w >> 16) as usize;
        let op = w & 0xffff;
        if n == 0 || i + n > words.len() {
            return Err("bad SPIR-V instruction".into());
        }
        let a = &words[i + 1..i + n];
        match op {
            15 => {
                mesh = match a[0] {
                    5364 => 1,
                    5365 => 2,
                    _ => 0,
                };
                stage = Some(match a[0] {
                    0 => 0,
                    1 => 3,
                    2 => 4,
                    3 => 2,
                    4 => 1,
                    5 => 5,
                    // NVN2's task and mesh shaders take the tessellation stages' places
                    5364 => 3,
                    5365 => 4,
                    m => return Err(format!("execution model {m}")),
                });
                let bytes: Vec<u8> = a[2..].iter().flat_map(|w| w.to_le_bytes()).collect();
                let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
                entry = String::from_utf8_lossy(&bytes[..end]).into_owned();
            }
            71 => match a[1] {
                34 => {
                    set.insert(a[0], a[2]);
                }
                33 => {
                    binding.insert(a[0], a[2]);
                }
                30 => {
                    location.insert(a[0], a[2]);
                }
                11 => {
                    builtin.insert(a[0], true);
                }
                _ => {}
            },
            32 => {
                ptr_types.insert(a[0], (a[1], a[2]));
            }
            21 | 22 | 23 | 28 | 30 => {
                types.insert(a[0], (op, a[1..].to_vec()));
            }
            59 => vars.push((a[1], a[0], a[2])),
            _ => {}
        }
        i += n;
    }
    let stage = stage.ok_or("no entry point")?;
    let scalar_kind = |mut t: u32| -> InKind {
        for _ in 0..4 {
            match types.get(&t) {
                Some((23, ops)) | Some((28, ops)) => t = ops[0],
                Some((21, ops)) => {
                    return if ops[1] == 1 {
                        InKind::Sint
                    } else {
                        InKind::Uint
                    }
                }
                _ => return InKind::Float,
            }
        }
        InKind::Float
    };
    let mut rv = Vec::new();
    let mut inputs = Vec::new();
    for &(id, ty, sc) in &vars {
        if let (Some(&s), Some(&b)) = (set.get(&id), binding.get(&id)) {
            rv.push((id, s, b));
        }
        if sc == 1 && stage == 0 && !builtin.contains_key(&id) {
            if let Some(&loc) = location.get(&id) {
                let pointee = ptr_types.get(&ty).map(|p| p.1).unwrap_or(0);
                inputs.push((loc, scalar_kind(pointee)));
            }
        }
    }
    Ok(Refl {
        stage,
        mesh,
        entry,
        vars: rv,
        inputs,
    })
}

/// Rewrites the DescriptorSet / Binding decorations of variable `id`.
fn redecorate(words: &mut [u32], id: u32, set: u32, binding: u32) {
    let mut i = 5;
    while i < words.len() {
        let n = (words[i] >> 16) as usize;
        if n == 0 {
            break;
        }
        if words[i] & 0xffff == 71 && n >= 4 && words[i + 1] == id {
            match words[i + 2] {
                34 => words[i + 3] = set,
                33 => words[i + 3] = binding,
                _ => {}
            }
        }
        i += n;
    }
}

/// One entry of lane C's `bindings[]`.
#[derive(serde::Deserialize, Debug)]
struct JBinding {
    set: u32,
    binding: u32,
    #[serde(rename = "type")]
    ty: String,
    #[serde(default)]
    count: u32,
    resource: serde_json::Value,
}
#[derive(serde::Deserialize, Debug, Default)]
struct JRefl {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    bindings: Vec<JBinding>,
    #[serde(default)]
    c1: Option<String>,
    #[serde(default)]
    frag_out_map: Option<i64>,
    #[serde(default)]
    transform_feedback: Option<serde_json::Value>,
    #[serde(default)]
    per_primitive_locations: Vec<u32>,
}

/// NXGFX_HANDLE_TABLES=0: shaders with handle-table texture arrays are refused as before.
fn handle_tables_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("NXGFX_HANDLE_TABLES").as_deref() != Ok("0"))
}

fn hex_bytes(h: &str) -> Option<Vec<u8>> {
    (0..h.len() / 2)
        .map(|i| u8::from_str_radix(&h[2 * i..2 * i + 2], 16).ok())
        .collect()
}

fn iv(v: &serde_json::Value, k: &str) -> i64 {
    v.get(k).and_then(|x| x.as_i64()).unwrap_or(0)
}

/// Moves a module's bindings into the shared layout. `json` = lane C's reflection; without one the module is taken
/// as written for the shared layout already. Returns the rewritten words and what to bind.
pub fn remap(
    words: &[u32],
    refl: &Refl,
    json: Option<&str>,
) -> Result<(Vec<u32>, Vec<(u32, Res)>, bool, Option<Vec<u8>>), String> {
    let mut w = words.to_vec();
    let st = refl.stage;
    let mut res = Vec::new();
    let mut pool = false;
    let Some(j) = json else {
        for &(_, s, b) in &refl.vars {
            if s == L::POOL_SET {
                pool = true;
                continue;
            }
            if s != st {
                return Err(format!(
                    "binding set {s} is not the module's stage set {st}"
                ));
            }
            let r = match b {
                0..=30 => Res::Cbuf { bank: b },
                31 => Res::Support,
                32..=63 => {
                    // the game's storage slot b - 32 in the stage's c0 table
                    let base = match st {
                        0 => 0x110,
                        5 => 0x310,
                        2 => 0x410,
                        1 => 0x510,
                        _ => 0,
                    };
                    Res::Sbuf {
                        cb_slot: 0,
                        cb_off: base + 0x10 * (b - 32),
                        written: true,
                    }
                }
                64..=127 => Res::Tex {
                    handle: (0x20 + 8 * (b - 64)) / 4,
                    cbuf_slot: -1,
                    ty: 3,
                },
                128..=143 => Res::Img {
                    handle: (0x120 + 8 * (b - 128)) / 4,
                    cbuf_slot: -1,
                    ty: 3,
                    fmt: 0,
                },
                _ => return Err(format!("binding {b} has no meaning without a reflection")),
            };
            res.push((b, r));
        }
        return Ok((w, res, pool, None));
    };
    let jr: JRefl = serde_json::from_str(j).map_err(|e| format!("reflection json: {e}"))?;
    if let Some(s) = &jr.status {
        if s != "ok" {
            return Err(format!("the shader build marked it {s}"));
        }
    }
    let (mut nsb, mut ntex, mut nimg, mut ntb, mut nib) = (0u32, 0u32, 0u32, 0u32, 0u32);
    let mut map: HashMap<(u32, u32), (u32, u32)> = HashMap::new();
    // handle tables: ((set, binding), constant buffer slot, first handle word), rewritten onto the pools below
    let mut tables: Vec<((u32, u32), i64, u32)> = Vec::new();
    for jb in &jr.bindings {
        let r = &jb.resource;
        let kind = r.get("kind").and_then(|k| k.as_str()).unwrap_or("");
        let array = iv(r, "array");
        if kind == "texture"
            && jb.ty == "combined_image_sampler"
            && array > 1
            && array != 1 << 20
            && array != 4096
            && handle_tables_on()
        {
            tables.push((
                (jb.set, jb.binding),
                iv(r, "cbuf_slot"),
                (iv(r, "handle") & 0x3fff) as u32,
            ));
            continue;
        }
        let (set, b, what) = match (kind, jb.ty.as_str()) {
            ("support", _) => (st, L::SUPPORT, Some(Res::Support)),
            ("mesh_draw", "storage_buffer") => {
                if nsb >= L::SSBO_N {
                    return Err("more than 32 storage buffers".into());
                }
                nsb += 1;
                (st, L::SSBO_BASE + nsb - 1, Some(Res::MeshDraw))
            }
            ("cbuf", "uniform_buffer") => {
                let bank = iv(r, "slot") as u32;
                if bank >= L::BANKS {
                    return Err(format!("constant bank {bank}"));
                }
                (st, bank, Some(Res::Cbuf { bank }))
            }
            ("sbuf", "storage_buffer") => {
                if nsb >= L::SSBO_N {
                    return Err("more than 32 storage buffers".into());
                }
                nsb += 1;
                (
                    st,
                    L::SSBO_BASE + nsb - 1,
                    Some(Res::Sbuf {
                        cb_slot: iv(r, "cb_slot") as i32,
                        cb_off: 4 * iv(r, "cb_offset") as u32,
                        written: iv(r, "flags") & 1 != 0,
                    }),
                )
            }
            (_, "sampled_image")
            | (_, "sampler")
            | (_, "storage_image")
            | (_, "uniform_texel_buffer")
            | (_, "storage_texel_buffer")
                if array == 1 << 20 || array == 4096 || jb.count == 0 =>
            {
                pool = true;
                let pb = match jb.ty.as_str() {
                    "sampled_image" => L::B_TEX,
                    "sampler" => L::B_SAMP,
                    "storage_image" => L::B_IMG,
                    "uniform_texel_buffer" => L::B_TBUF,
                    _ => L::B_IBUF,
                };
                (L::POOL_SET, pb, None)
            }
            ("texture", "combined_image_sampler") => {
                if array != 1 && array != 0 {
                    return Err(format!(
                        "texture array of {array} (handle tables) is not bound yet"
                    ));
                }
                if ntex >= L::TEX_N {
                    return Err("more than 64 textures".into());
                }
                ntex += 1;
                (
                    st,
                    L::TEX_BASE + ntex - 1,
                    Some(Res::Tex {
                        handle: iv(r, "handle") as u32,
                        cbuf_slot: iv(r, "cbuf_slot") as i32,
                        ty: iv(r, "type") as u32,
                    }),
                )
            }
            ("texture", "uniform_texel_buffer") => {
                ntb += 1;
                (
                    st,
                    L::TBUF_BASE + ntb - 1,
                    Some(Res::Tex {
                        handle: iv(r, "handle") as u32,
                        cbuf_slot: iv(r, "cbuf_slot") as i32,
                        ty: 2,
                    }),
                )
            }
            ("image", "storage_image") => {
                nimg += 1;
                let fmt = r
                    .get("format")
                    .and_then(|f| f.as_str())
                    .map(image_format_code)
                    .unwrap_or(0);
                (
                    st,
                    L::IMG_BASE + nimg - 1,
                    Some(Res::Img {
                        handle: iv(r, "handle") as u32,
                        cbuf_slot: iv(r, "cbuf_slot") as i32,
                        ty: iv(r, "type") as u32,
                        fmt,
                    }),
                )
            }
            ("image", "storage_texel_buffer") => {
                nib += 1;
                (
                    st,
                    L::IBUF_BASE + nib - 1,
                    Some(Res::Img {
                        handle: iv(r, "handle") as u32,
                        cbuf_slot: iv(r, "cbuf_slot") as i32,
                        ty: 2,
                        fmt: 0,
                    }),
                )
            }
            (k, t) => return Err(format!("binding kind {k} as {t} is not bound yet")),
        };
        if ntb > L::XBUF_N || nib > L::XBUF_N || nimg > L::IMG_N {
            return Err("too many images or buffer textures".into());
        }
        map.insert((jb.set, jb.binding), (set, b));
        if let Some(what) = what {
            res.push((b, what));
        }
    }
    let var_at = |set: u32, binding: u32| {
        refl.vars
            .iter()
            .find(|v| v.1 == set && v.2 == binding)
            .map(|v| v.0)
    };
    let mut ht = Vec::new();
    for &((set, binding), slot, first_word) in &tables {
        let Some(var) = var_at(set, binding) else {
            return Err(format!(
                "handle table at set {set} binding {binding} has no variable"
            ));
        };
        let cb = jr.bindings.iter().find(|b| {
            b.resource.get("kind").and_then(|k| k.as_str()) == Some("cbuf")
                && iv(&b.resource, "slot") == slot
        });
        let Some(cbuf) = cb.and_then(|b| var_at(b.set, b.binding)) else {
            return Err(format!("texture array (handle table in c{slot}) whose constant buffer the shader does not declare"));
        };
        ht.push(shaders::link::HandleTable {
            var,
            cbuf,
            first_word,
        });
    }
    for &(id, s, b) in &refl.vars {
        if ht.iter().any(|t| t.var == id) {
            continue;
        }
        match map.get(&(s, b)) {
            Some(&(ns, nb)) => redecorate(&mut w, id, ns, nb),
            None => {
                return Err(format!(
                    "variable at set {s} binding {b} is not in the reflection"
                ))
            }
        }
    }
    if !ht.is_empty() {
        let caps = super::Gpu::try_get()
            .map(|g| (g.layout.tex_cap, g.layout.samp_cap))
            .unwrap_or((262144, 16384));
        w = shaders::link::handle_tables(&w, &ht, L::POOL_SET, L::B_TEX, L::B_SAMP, caps.0, caps.1)
            .ok_or("a texture array (handle table) used other than by loading its elements")?;
        pool = true;
        static SAID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        if SAID.fetch_add(1, std::sync::atomic::Ordering::Relaxed) == 0 {
            crate::vklog!("texture arrays read through handle tables in constant buffers: sampled from the texture and sampler pools by the handles (NXGFX_HANDLE_TABLES=0 off)");
        }
    }
    let c1 = jr.c1.as_deref().and_then(hex_bytes);
    Ok((w, res, pool, c1))
}

pub struct Library {
    dir: Mutex<Option<PathBuf>>,
    mem: Mutex<HashMap<ShaderKey, (Vec<u32>, Option<String>)>>,
    modules: Mutex<HashMap<ShaderKey, Option<Arc<Module>>>>,
    /// keys found in no folder, and when they were last looked for (looked for again twice a second: the live
    /// translator may deliver them)
    missing: Mutex<HashMap<ShaderKey, std::time::Instant>>,
    next_id: std::sync::atomic::AtomicU64,
}

/// Effective variants have their own domain, with fixed-width little-endian lengths/words. Keep the original
/// shader keys unchanged. Including the resulting SPIR-V also distinguishes changed rewrites of the same recipe.
pub(super) fn variant_key(base: &ShaderKey, what: u8, tag: &[u32], words: &[u32]) -> ShaderKey {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(b"nxgfx/effective-shader-variant/v1\0");
    h.update(base);
    h.update([what]);
    for values in [tag, words] {
        h.update((values.len() as u64).to_le_bytes());
        for word in values {
            h.update(word.to_le_bytes());
        }
    }
    let mut key = [0; 8];
    key.copy_from_slice(&h.finalize()[..8]);
    key
}

/// The old 16-bit recipe is ONLY for honoring existing compiler blacklists, never a runtime cache identity.
fn old_variant_key(base: &ShaderKey, what: u8, tag: &[u32]) -> ShaderKey {
    let mut key = *base;
    let h = tag.iter().fold(0x811c_9dc5u32 ^ what as u32, |a, &x| {
        (a ^ x).wrapping_mul(0x0100_0193)
    });
    key[5] ^= 0x96 ^ what;
    key[6] ^= (h >> 8) as u8;
    key[7] ^= h as u8;
    key
}

static LEGACY_VARIANTS: std::sync::LazyLock<Mutex<HashMap<ShaderKey, ShaderKey>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

pub(super) fn legacy_shader_key(key: &ShaderKey) -> ShaderKey {
    LEGACY_VARIANTS
        .lock()
        .unwrap()
        .get(key)
        .copied()
        .unwrap_or(*key)
}

pub(super) fn note_variant_key(key: ShaderKey, base: &ShaderKey, what: u8, tag: &[u32]) {
    let mut aliases = LEGACY_VARIANTS.lock().unwrap();
    let old_base = aliases.get(base).copied().unwrap_or(*base);
    let old = old_variant_key(&old_base, what, tag);
    aliases.insert(key, old);
    // xfb::variant_module derives its key by this XOR. Preserve blacklist identity for an input-retyped XFB
    // variant too, including when link_io subsequently adds outputs to that capture variant.
    let (mut xfb, mut old_xfb) = (key, old);
    xfb[7] ^= 0x5a;
    old_xfb[7] ^= 0x5a;
    aliases.insert(xfb, old_xfb);
}

impl Library {
    pub fn new() -> Library {
        Library {
            dir: Mutex::new(std::env::var("NXGFX_SHADERS").ok().map(PathBuf::from)),
            mem: Mutex::new(HashMap::new()),
            modules: Mutex::new(HashMap::new()),
            missing: Mutex::new(HashMap::new()),
            next_id: std::sync::atomic::AtomicU64::new(1),
        }
    }
    /// The game's `<data>/shaders`.
    pub fn set_dir(&self, p: PathBuf) {
        *self.dir.lock().unwrap() = Some(p);
    }
    /// The game's `<data>/shaders`, when set.
    pub fn dir(&self) -> Option<PathBuf> {
        self.dir.lock().unwrap().clone()
    }
    /// A module in memory under `key`, with lane C's reflection json (None: written for the shared layout).
    pub fn insert(&self, key: ShaderKey, spirv: Vec<u32>, json: Option<String>) {
        self.mem.lock().unwrap().insert(key, (spirv, json));
        self.modules.lock().unwrap().remove(&key);
    }

    fn load(&self, key: &ShaderKey) -> Option<(Vec<u32>, Option<String>)> {
        if let Some(v) = self.mem.lock().unwrap().get(key) {
            return Some(v.clone());
        }
        let dir = self.dir.lock().unwrap().clone();
        let hex = key_hex(key);
        let dirs: Vec<PathBuf> = dir
            .iter()
            .flat_map(|d| [d.join("spv"), d.clone()])
            .chain(super::live::spv_dir(key))
            .collect();
        for d in dirs {
            if let Ok(bytes) = std::fs::read(d.join(format!("{hex}.spv"))) {
                if bytes.len() % 4 != 0 {
                    crate::log::red(&format!("shader {hex}.spv is not whole words"));
                    return None;
                }
                let words = bytes
                    .chunks_exact(4)
                    .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                    .collect();
                let json = std::fs::read_to_string(d.join(format!("{hex}.json"))).ok();
                return Some((words, json));
            }
        }
        None
    }

    /// The module of a shader key; None (a red line once) when the library has none or it cannot be bound.
    pub fn get(&self, dev: &ash::Device, key: &ShaderKey) -> Option<Arc<Module>> {
        self.get_ex(dev, key, false)
    }

    /// Whether a module of `key` is on disk (or in memory) now.
    pub fn has(&self, key: &ShaderKey) -> bool {
        if self.mem.lock().unwrap().contains_key(key)
            || self
                .modules
                .lock()
                .unwrap()
                .get(key)
                .is_some_and(|m| m.is_some())
        {
            return true;
        }
        let Some(dir) = self.dir.lock().unwrap().clone() else {
            return false;
        };
        let hex = key_hex(key);
        dir.join("spv").join(format!("{hex}.spv")).is_file()
            || dir.join(format!("{hex}.spv")).is_file()
            || super::live::spv_dir(key).is_some()
    }

    /// Whether `key` was loaded and refused (its module cannot be built): waiting for it is pointless.
    pub fn refused(&self, key: &ShaderKey) -> bool {
        self.modules
            .lock()
            .unwrap()
            .get(key)
            .is_some_and(|m| m.is_none())
    }

    /// As `get`; `force`: a missing key is looked for again now (a draw waiting for the live translator).
    pub fn get_ex(&self, dev: &ash::Device, key: &ShaderKey, force: bool) -> Option<Arc<Module>> {
        if let Some(m) = self.modules.lock().unwrap().get(key) {
            return m.clone();
        }
        if let Some(t) = self.missing.lock().unwrap().get(key) {
            if !force && t.elapsed() < std::time::Duration::from_millis(500) {
                return None;
            }
        }
        let built = match self.load(key) {
            None => {
                crate::log::red_once(&format!(
                    "shader {} is not in the shader library",
                    key_hex(key)
                ));
                self.missing
                    .lock()
                    .unwrap()
                    .insert(*key, std::time::Instant::now());
                return None;
            }
            Some((w, j)) => match self.build(dev, key, &w, j.as_deref()) {
                Ok(m) => Some(m),
                Err(e) => {
                    crate::log::red_once(&format!("shader {}: {e}", key_hex(key)));
                    None
                }
            },
        };
        if built.is_some() && self.missing.lock().unwrap().remove(key).is_some() {
            crate::vklog!("shader {} is in the library now", key_hex(key));
        }
        self.modules.lock().unwrap().insert(*key, built.clone());
        built
    }

    /// The pixel shader `key` with its inputs at `locs` read per primitive (drawn with a mesh shader that writes
    /// them so; lane S's `shaders::mesh::fragment_per_primitive`), made once per (key, locations).
    pub fn per_primitive(
        &self,
        dev: &ash::Device,
        key: &ShaderKey,
        locs: &[u32],
    ) -> Option<Arc<Module>> {
        let mut vk = *key;
        vk[6] ^= 0xc3;
        vk[7] ^= locs.iter().fold(0u8, |a, &l| a ^ (1u8 << (l & 7)));
        if let Some(m) = self.modules.lock().unwrap().get(&vk) {
            return m.clone();
        }
        let built = self.load(key).and_then(|(w, j)| {
            let bytes: Vec<u8> = w.iter().flat_map(|x| x.to_le_bytes()).collect();
            match shaders::mesh::fragment_per_primitive(&bytes, locs) {
                Ok(b) => {
                    let w2: Vec<u32> = b
                        .chunks_exact(4)
                        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                        .collect();
                    match self.build(dev, &vk, &w2, j.as_deref()) {
                        Ok(m) => Some(m),
                        Err(e) => {
                            crate::log::red_once(&format!(
                                "pixel shader {}: per-primitive variant: {e}",
                                key_hex(key)
                            ));
                            None
                        }
                    }
                }
                Err(e) => {
                    crate::log::red_once(&format!(
                        "pixel shader {}: no per-primitive variant ({e}): drawn as built",
                        key_hex(key)
                    ));
                    None
                }
            }
        });
        self.modules.lock().unwrap().insert(vk, built.clone());
        built
    }

    /// Module `m` (the stage before the pixel shader) with outputs added for the varyings `add` its pixel shader reads
    /// and it never writes (`shaders::link::add_outputs`: (0, 0, 0, 1)); made once per (module, varyings).
    pub fn with_outputs(
        &self,
        dev: &ash::Device,
        m: &Arc<Module>,
        add: &[shaders::link::Varying],
    ) -> Option<Arc<Module>> {
        let tag: Vec<u32> = add
            .iter()
            .flat_map(|v| [v.location, v.component, v.kind as u32, v.components])
            .collect();
        let made = self.variant(dev, m, 1, &tag, || {
            shaders::link::add_outputs(&m.words, add)
        });
        if made.is_none() {
            crate::log::red_once(&format!(
                "shader {}: the varyings its pixel shader reads could not be added",
                key_hex(&m.key)
            ));
        }
        made
    }

    /// Pixel shader `m` with its outputs at the locations of integer targets retyped (`kinds`: 1 unsigned, 2 signed;
    /// `shaders::link::int_outputs`, Turbo's integer output variant); made once per (module, kinds).
    pub fn with_int_outputs(
        &self,
        dev: &ash::Device,
        m: &Arc<Module>,
        kinds: &[u8; 8],
    ) -> Option<Arc<Module>> {
        let tag: Vec<u32> = kinds.iter().map(|&k| k as u32).collect();
        let made = self.variant(dev, m, 2, &tag, || {
            shaders::link::int_outputs(&m.words, kinds)
        });
        if made.is_none() {
            crate::log::red_once(&format!("shader {}: no integer-output variant (its float stores could not be retyped): the values written to integer targets stay undefined", key_hex(&m.key)));
        }
        made
    }

    /// Pixel shader `m` for a draw's blend and alpha state: `dual`, its output 1 as target 0's second source
    /// (`shaders::link::dual_source`); `alpha`, NVN's alpha test before each return (`shaders::link::alpha_test`, the
    /// function and reference in push constants). None when it cannot be made (said in red once per shader).
    pub fn fragment_variant(
        &self,
        dev: &ash::Device,
        m: &Arc<Module>,
        dual: bool,
        alpha: bool,
    ) -> Option<Arc<Module>> {
        let made = self.variant(dev, m, 3, &[dual as u32, alpha as u32], || {
            let w = if dual {
                shaders::link::dual_source(&m.words)?
            } else {
                m.words.clone()
            };
            if alpha {
                shaders::link::alpha_test(&w)
            } else {
                Some(w)
            }
        });
        if made.is_none() {
            let what = match (dual, alpha) {
                (true, true) => "dual-source + alpha test",
                (true, false) => "dual-source (it does not write exactly outputs 0 and 1: the second source counts as zero)",
                _ => "alpha test (its colour 0 is not a float vec4, or its push block cannot take the test: drawn without it)",
            };
            crate::log::red_once(&format!(
                "pixel shader {}: no {what} variant",
                key_hex(&m.key)
            ));
        }
        made
    }

    /// Vertex fetch with matching scalar classes, preserving the original register bits in the shader body.
    pub fn with_vertex_inputs(
        &self,
        dev: &ash::Device,
        m: &Arc<Module>,
        inputs: &[(u32, InKind)],
    ) -> Option<Arc<Module>> {
        let tag: Vec<u32> = inputs
            .iter()
            .flat_map(|&(loc, kind)| {
                [
                    loc,
                    match kind {
                        InKind::Float => 0,
                        InKind::Uint => 1,
                        InKind::Sint => 2,
                    },
                ]
            })
            .collect();
        let made = self.variant(dev, m, 4, &tag, || retype_vertex_inputs(&m.words, inputs));
        if made.is_none() {
            crate::log::red_once(&format!(
                "vertex shader {}: cannot retype mismatched input classes {inputs:?}",
                key_hex(&m.key)
            ));
        } else {
            static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            if N.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 16 {
                crate::vklog!("vertex shader {}: input classes {inputs:?} matched to vertex formats, register bits preserved", key_hex(&m.key));
            }
        }
        made
    }

    /// A variant of module `m` made from the words `make` gives (what: 1 added outputs, 2 integer outputs, 4 inputs; `tag` tells
    /// variants of one kind apart), cached; its key differs from the shader's (pipeline parts and caches key on it).
    fn variant(
        &self,
        dev: &ash::Device,
        m: &Arc<Module>,
        what: u8,
        tag: &[u32],
        make: impl FnOnce() -> Option<Vec<u32>>,
    ) -> Option<Arc<Module>> {
        type K = (u64, u8, Vec<u32>);
        static MADE: Mutex<Option<HashMap<K, Option<Arc<Module>>>>> = Mutex::new(None);
        let k: K = (m.id, what, tag.to_vec());
        if let Some(v) = MADE
            .lock()
            .unwrap()
            .get_or_insert_with(HashMap::new)
            .get(&k)
        {
            return v.clone();
        }
        let made = make().and_then(|w| {
            let inputs = reflect(&w).ok()?.inputs;
            let ci = vk::ShaderModuleCreateInfo::default().code(&w);
            let module = unsafe { dev.create_shader_module(&ci, None) }.ok()?;
            let key = variant_key(&m.key, what, tag, &w);
            note_variant_key(key, &m.key, what, tag);
            Some(Arc::new(Module {
                key,
                stage: m.stage,
                module,
                entry: m.entry.clone(),
                res: m.res.clone(),
                uses_pool: m.uses_pool,
                inputs,
                c1: m.c1.clone(),
                id: self
                    .next_id
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
                omap: m.omap,
                mesh: m.mesh,
                xfb: m.xfb.clone(),
                words: w,
                per_primitive: m.per_primitive.clone(),
            }))
        });
        MADE.lock()
            .unwrap()
            .get_or_insert_with(HashMap::new)
            .insert(k, made.clone());
        made
    }

    fn build(
        &self,
        dev: &ash::Device,
        key: &ShaderKey,
        words: &[u32],
        json: Option<&str>,
    ) -> Result<Arc<Module>, String> {
        let r = reflect(words)?;
        let (w, res, pool, c1) = remap(words, &r, json)?;
        // a vertex shader's outputs written once at the end (`spvfix`; NXGFX_LATE_OUTPUTS=0: as translated)
        let w = if r.stage == 0
            && r.mesh == 0
            && std::env::var("NXGFX_LATE_OUTPUTS").as_deref() != Ok("0")
        {
            super::spvfix::late_outputs(&w).unwrap_or(w)
        } else {
            w
        };
        // a discard made a demote to a helper: NVIDIA lost the device on a pass whose fragment shader ends pixels
        // midway (mkw-recomp #22 / #231; NXGFX_DEMOTE_FIX=0 off)
        let w = if r.stage == 1
            && std::env::var("NXGFX_DEMOTE_FIX").as_deref() != Ok("0")
            && super::spvfix::DEMOTE.load(std::sync::atomic::Ordering::Relaxed)
        {
            super::spvfix::kill_demote(&w).unwrap_or(w)
        } else {
            w
        };
        // integer abs as a product, which NVIDIA's compiler does not turn into a GPU fault (mkw-recomp #395 / #22;
        // NXGFX_SABS_FIX=0 off)
        let w = if std::env::var("NXGFX_SABS_FIX").as_deref() != Ok("0") {
            super::spvfix::sabs_product(&w).unwrap_or(w)
        } else {
            w
        };
        // primitive indices held below the vertex count (NXGFX_MESH_INDEX_FIX=0 off; MKW's mesh-draw device losses)
        let w = if r.mesh == 2 && std::env::var("NXGFX_MESH_INDEX_FIX").as_deref() != Ok("0") {
            super::spvfix::mesh_indices(&w).unwrap_or(w)
        } else {
            w
        };
        // task / mesh shared memory starts zeroed (NXGFX_ZERO_SHARED=0 off; the device's shaderZeroInitializeWorkgroupMemory)
        let w = if r.mesh != 0
            && std::env::var("NXGFX_ZERO_SHARED").as_deref() != Ok("0")
            && super::spvfix::ZERO_WG.load(std::sync::atomic::Ordering::Relaxed)
        {
            super::spvfix::zero_workgroup(&w).unwrap_or(w)
        } else {
            w
        };
        // NVN's warp-aggregated atomic add made per lane (the RTX 50 ballot hang, Turbo #12; NXGFX_BALLOT_FIX=0 off)
        let w = if std::env::var("NXGFX_BALLOT_FIX").as_deref() != Ok("0") {
            match shaders::link::ballot_per_lane(&w) {
                Some((nw, n)) => {
                    crate::vklog!("shader {}: {n} subgroup ballot(s) for a counter made per lane (RTX 50 hang)", key_hex(key));
                    nw
                }
                None => w,
            }
        } else {
            w
        };
        // every loop bounded (GPU hangs: a loop whose exit depends on garbage never ends and the driver resets the GPU);
        // NXGFX_LOOP_GUARD=<back-edges a call> (default 65536), 0 off
        let w = match loop_cap() {
            0 => w,
            cap => match shaders::link::loop_guard(&w, cap) {
                Some((nw, _)) => nw,
                None => w,
            },
        };
        // gl_PerVertex's unused cull member dropped when clip + cull pass the device's limit (shaders::link)
        let limit = CLIP_CULL_LIMIT.load(std::sync::atomic::Ordering::Relaxed);
        let w = if r.stage != 1 && r.stage != 5 && limit != 0 {
            match shaders::link::trim_cull(&w, limit) {
                Some(t) => t,
                None => {
                    if shaders::link::needs_trim(&w, limit) {
                        crate::log::red_once(&format!("shader {}: more clip and cull distances than the device takes, and the cull ones are used", key_hex(key)));
                    }
                    w
                }
            }
        } else {
            w
        };
        let jr = json.and_then(|j| serde_json::from_str::<JRefl>(j).ok());
        let omap = jr
            .as_ref()
            .and_then(|j| j.frag_out_map)
            .filter(|&m| m >= 0 && r.stage == 1)
            .map(|m| m as u32);
        let xfb = jr
            .as_ref()
            .and_then(|j| j.transform_feedback.as_ref())
            .and_then(super::xfb::Layout::from_json);
        let ci = vk::ShaderModuleCreateInfo::default().code(&w);
        let module = unsafe { dev.create_shader_module(&ci, None) }
            .map_err(|e| format!("vkCreateShaderModule: {e}"))?;
        // Every source path (disk, live cache, in-memory insertion, prewarm, and keyed build variants) converges here.
        super::capture::shader_module(*key, r.stage, words, json);
        Ok(Arc::new(Module {
            key: *key,
            stage: r.stage,
            module,
            entry: std::ffi::CString::new(r.entry).unwrap(),
            res,
            uses_pool: pool,
            inputs: r.inputs,
            c1,
            omap,
            mesh: r.mesh,
            xfb,
            words: w.clone(),
            per_primitive: jr
                .as_ref()
                .map(|j| j.per_primitive_locations.clone())
                .unwrap_or_default(),
            id: self
                .next_id
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        }))
    }
}

impl Default for Library {
    fn default() -> Self {
        Self::new()
    }
}

/// The NVN format code of a storage image format the translator declares (its `format` names, SPIR-V's image
/// formats); 0 for `Unknown` and the formats a view need not change (normalized and float ones read through the
/// texture's own format).
fn image_format_code(f: &str) -> u16 {
    match f {
        "R32Uint" => 11,
        "R32Sint" => 12,
        "Rg32Uint" => 23,
        "Rg32Sint" => 24,
        "Rgba32Uint" => 47,
        "Rgba32Sint" => 48,
        "R16Uint" => 8,
        "R16Sint" => 9,
        "Rg16Uint" => 20,
        "Rg16Sint" => 21,
        "Rgba16Uint" => 44,
        "Rgba16Sint" => 45,
        "R8Uint" => 3,
        "R8Sint" => 4,
        "Rg8Uint" => 15,
        "Rg8Sint" => 16,
        "Rgba8Uint" => 39,
        "Rgba8Sint" => 40,
        _ => 0,
    }
}

#[cfg(test)]
mod variant_identity_tests {
    use super::*;

    const BASE: ShaderKey = [0, 1, 2, 3, 4, 5, 6, 7];
    const WORDS: [u32; 5] = [0x0723_0203, 0x10000, 0, 1, 0];

    fn formerly_colliding(what: u8, a: &[u32], b: &[u32]) {
        assert_eq!(
            old_variant_key(&BASE, what, a),
            old_variant_key(&BASE, what, b)
        );
        let ka = variant_key(&BASE, what, a, &WORDS);
        let kb = variant_key(&BASE, what, b, &WORDS);
        assert_ne!(ka, kb);
        assert_ne!(ka, BASE);
        assert_ne!(kb, BASE);
        assert_eq!(ka, variant_key(&BASE, what, &a.to_vec(), &WORDS.to_vec()));
        assert_eq!(kb, variant_key(&BASE, what, &b.to_vec(), &WORDS.to_vec()));
    }

    #[test]
    fn integer_output_tags_no_longer_collide() {
        formerly_colliding(2, &[0, 0, 0, 0, 0, 0, 2, 1], &[0, 1, 0, 0, 0, 2, 0, 0]);
    }

    #[test]
    fn vertex_input_tags_no_longer_collide() {
        formerly_colliding(4, &[0, 1, 12, 2], &[12, 2, 16, 1]);
        // Pinned byte encoding: independent of process, module IDs, native endianness and HashMap seeds.
        assert_eq!(
            variant_key(&BASE, 4, &[0, 1, 12, 2], &WORDS),
            [0xf8, 0xba, 0x89, 0x1f, 0x98, 0x9b, 0x66, 0x53]
        );
    }

    #[test]
    fn identity_covers_base_kind_full_tag_and_result_with_separate_lengths() {
        let key = variant_key(&BASE, 4, &[1, 2], &WORDS);
        assert_ne!(key, variant_key(&[9; 8], 4, &[1, 2], &WORDS));
        assert_ne!(key, variant_key(&BASE, 2, &[1, 2], &WORDS));
        assert_ne!(key, variant_key(&BASE, 4, &[2, 1], &WORDS));
        assert_ne!(key, variant_key(&BASE, 4, &[1, 0x10002], &WORDS));
        assert_ne!(
            key,
            variant_key(&BASE, 4, &[1, 2], &[0x0723_0203, 0x10000, 0, 2, 0])
        );
        assert_ne!(
            variant_key(&BASE, 4, &[1], &[2, 3]),
            variant_key(&BASE, 4, &[1, 2], &[3])
        );
    }

    #[test]
    fn legacy_blacklist_alias_survives_chained_and_xfb_variants() {
        let tag = [3, 1, 4, 1];
        let input = variant_key(&BASE, 4, &tag, &WORDS);
        note_variant_key(input, &BASE, 4, &tag);
        let old_input = old_variant_key(&BASE, 4, &tag);
        assert_eq!(legacy_shader_key(&input), old_input);
        let (mut xfb, mut old_xfb) = (input, old_input);
        xfb[7] ^= 0x5a;
        old_xfb[7] ^= 0x5a;
        assert_eq!(legacy_shader_key(&xfb), old_xfb);
        let outputs = [7, 0, 0, 4];
        let linked = variant_key(&xfb, 1, &outputs, &WORDS);
        note_variant_key(linked, &xfb, 1, &outputs);
        assert_eq!(
            legacy_shader_key(&linked),
            old_variant_key(&old_xfb, 1, &outputs)
        );
        assert_eq!(
            legacy_shader_key(&BASE),
            BASE,
            "original shader keys stay unchanged"
        );
    }
}

#[cfg(test)]
mod vertex_input_tests {
    use super::*;

    fn ins(w: &mut Vec<u32>, op: u32, a: &[u32]) {
        w.push(((a.len() as u32 + 1) << 16) | op);
        w.extend_from_slice(a);
    }

    fn fixture(escape: bool) -> Vec<u32> {
        let mut w = vec![0x0723_0203, 0x0001_0300, 0, 20, 0];
        ins(&mut w, 17, &[1]); // Shader
        ins(&mut w, 14, &[0, 1]); // Logical GLSL450
        ins(&mut w, 15, &[0, 13, 0x6e69_616d, 0, 8, 11, 12]);
        ins(&mut w, 71, &[8, 30, 3]); // input location 3
        ins(&mut w, 71, &[11, 30, 3]); // output at the SAME location must not be retyped
        ins(&mut w, 71, &[12, 11, 0]); // Position
        ins(&mut w, 19, &[1]);
        ins(&mut w, 33, &[2, 1]);
        ins(&mut w, 22, &[3, 32]);
        ins(&mut w, 23, &[4, 3, 4]);
        ins(&mut w, 32, &[7, 1, 4]);
        ins(&mut w, 59, &[7, 8, 1]);
        // The fetch scalar type is declared after the original input variable; moving the affected variable
        // must preserve declaration ordering without making duplicate scalar types.
        ins(&mut w, 21, &[5, 32, 0]);
        ins(&mut w, 43, &[5, 6, 0]);
        ins(&mut w, 32, &[9, 1, 3]);
        ins(&mut w, 32, &[10, 3, 4]);
        ins(&mut w, 59, &[10, 11, 3]);
        ins(&mut w, 59, &[10, 12, 3]);
        ins(&mut w, 54, &[1, 13, 0, 2]);
        ins(&mut w, 248, &[14]);
        if escape {
            ins(&mut w, 83, &[7, 18, 8]);
        }
        ins(&mut w, 61, &[4, 15, 8]); // whole vector load
        ins(&mut w, 65, &[9, 16, 8, 6]);
        ins(&mut w, 61, &[3, 17, 16]); // scalar component load
        ins(&mut w, 62, &[11, 15]);
        ins(&mut w, 62, &[12, 15]);
        ins(&mut w, 253, &[]);
        ins(&mut w, 56, &[]);
        w
    }

    fn instructions(w: &[u32]) -> Vec<&[u32]> {
        let mut out = Vec::new();
        let mut i = 5;
        while i < w.len() {
            let n = (w[i] >> 16) as usize;
            assert!(n > 0 && i + n <= w.len());
            out.push(&w[i..i + n]);
            i += n;
        }
        out
    }

    #[test]
    fn dkb_fetch_formats_require_two_integer_inputs() {
        for (format, kind) in [
            (0x22, InKind::Float),
            (0x69, InKind::Float),
            (0x27, InKind::Uint),
            (0x0f, InKind::Uint),
        ] {
            assert_eq!(
                vertex_fetch_kind(crate::tex::format::vertex_format(format)),
                Some(kind)
            );
        }
        assert_eq!(
            vertex_fetch_kind(vk::Format::R16G16_SINT),
            Some(InKind::Sint)
        );
        assert_eq!(
            vertex_fetch_kind(vk::Format::R16G16_SSCALED),
            Some(InKind::Float)
        );
    }

    #[test]
    fn typed_fetch_bitcasts_vector_and_component_loads_without_changing_outputs() {
        let w = retype_vertex_inputs(&fixture(false), &[(3, InKind::Uint)]).unwrap();
        assert_eq!(reflect(&w).unwrap().inputs, [(3, InKind::Uint)]);
        let ops = instructions(&w);
        assert!(ops.iter().any(|a| *a == [4 << 16 | 59, 10, 11, 3]));
        let casts: Vec<_> = ops.iter().filter(|a| a[0] & 0xffff == 124).collect();
        assert_eq!(casts.len(), 2);
        assert_eq!((casts[0][1], casts[0][2]), (4, 15));
        assert_eq!((casts[1][1], casts[1][2]), (3, 17));
        assert!(
            !ops.iter().any(|a| matches!(a[0] & 0xffff, 109..=112)),
            "fetch must keep bits, not convert values"
        );
    }

    #[test]
    fn signed_and_float_fetches_work_and_pointer_escapes_are_refused() {
        let w = retype_vertex_inputs(&fixture(false), &[(3, InKind::Sint)]).unwrap();
        assert_eq!(reflect(&w).unwrap().inputs, [(3, InKind::Sint)]);
        let w = retype_vertex_inputs(&w, &[(3, InKind::Float)]).unwrap();
        assert_eq!(reflect(&w).unwrap().inputs, [(3, InKind::Float)]);
        assert!(retype_vertex_inputs(&fixture(false), &[(3, InKind::Float)]).is_none());
        assert!(retype_vertex_inputs(&fixture(true), &[(3, InKind::Uint)]).is_none());
    }

    #[test]
    #[ignore = "CPU-only: requires DKB shader artifacts and spirv-val (NXGFX_SPIRV_VAL)"]
    fn dkb_vertex_input_variants_validate_offline() {
        let dir = std::env::var_os("NXGFX_VIN_TEST_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../../build/games/dkb/dkb_data/shaders/spv")
            });
        let val = std::env::var_os("NXGFX_SPIRV_VAL").unwrap_or_else(|| "spirv-val".into());
        for key in ["d661413914c84d55", "f9c807930abd1234"] {
            let data = std::fs::read(dir.join(format!("{key}.spv"))).unwrap();
            let words: Vec<_> = data
                .chunks_exact(4)
                .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
                .collect();
            let words = super::super::spvfix::late_outputs(&words).unwrap_or(words);
            let words = shaders::link::trim_cull(&words, 8).unwrap_or(words);
            let changed =
                retype_vertex_inputs(&words, &[(3, InKind::Uint), (4, InKind::Uint)]).unwrap();
            let mut inputs = reflect(&changed).unwrap().inputs;
            inputs.sort_by_key(|&(l, _)| l);
            assert_eq!(
                inputs,
                [
                    (0, InKind::Float),
                    (1, InKind::Float),
                    (3, InKind::Uint),
                    (4, InKind::Uint)
                ]
            );
            let file =
                std::env::temp_dir().join(format!("nxgfx-vin-{}-{key}.spv", std::process::id()));
            let bytes: Vec<_> = changed.iter().flat_map(|w| w.to_le_bytes()).collect();
            std::fs::write(&file, bytes).unwrap();
            let result = std::process::Command::new(&val)
                .args(["--target-env", "vulkan1.3"])
                .arg(&file)
                .output();
            let _ = std::fs::remove_file(&file);
            let result = result.unwrap();
            assert!(
                result.status.success(),
                "{key}: {}{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
    }
}
