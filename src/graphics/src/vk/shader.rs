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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InKind {
    Float,
    Sint,
    Uint,
}

/// What the host binds at one binding of a module's stage set.
#[derive(Clone, Debug, PartialEq)]
pub enum Res {
    /// constant bank c[bank]
    Cbuf { bank: u32 },
    Support,
    /// a storage buffer whose {u64 address, u32 size} is c[cb_slot] at byte `cb_off`
    Sbuf { cb_slot: i32, cb_off: u32, written: bool },
    /// a texture whose handle word(s) are c[cbuf_slot] (-1 = c2) at word `handle & 0x3fff` (sampler word at
    /// `handle >> 14 & 0x3fff` for a separate sampler handle); `ty` is the sampler type (dimension | array 0x100)
    Tex { handle: u32, cbuf_slot: i32, ty: u32 },
    Img { handle: u32, cbuf_slot: i32, ty: u32 },
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
                Some((21, ops)) => return if ops[1] == 1 { InKind::Sint } else { InKind::Uint },
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
    Ok(Refl { stage, mesh, entry, vars: rv, inputs })
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

fn hex_bytes(h: &str) -> Option<Vec<u8>> {
    (0..h.len() / 2).map(|i| u8::from_str_radix(&h[2 * i..2 * i + 2], 16).ok()).collect()
}

fn iv(v: &serde_json::Value, k: &str) -> i64 {
    v.get(k).and_then(|x| x.as_i64()).unwrap_or(0)
}

/// Moves a module's bindings into the shared layout. `json` = lane C's reflection; without one the module is taken
/// as written for the shared layout already. Returns the rewritten words and what to bind.
pub fn remap(words: &[u32], refl: &Refl, json: Option<&str>) -> Result<(Vec<u32>, Vec<(u32, Res)>, bool, Option<Vec<u8>>), String> {
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
                return Err(format!("binding set {s} is not the module's stage set {st}"));
            }
            let r = match b {
                0..=30 => Res::Cbuf { bank: b },
                31 => Res::Support,
                32..=63 => {
                    // the game's storage slot b - 32 in the stage's c0 table
                    let base = match st { 0 => 0x110, 5 => 0x310, 2 => 0x410, 1 => 0x510, _ => 0 };
                    Res::Sbuf { cb_slot: 0, cb_off: base + 0x10 * (b - 32), written: true }
                }
                64..=127 => Res::Tex { handle: (0x20 + 8 * (b - 64)) / 4, cbuf_slot: -1, ty: 3 },
                128..=143 => Res::Img { handle: (0x120 + 8 * (b - 128)) / 4, cbuf_slot: -1, ty: 3 },
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
    for jb in &jr.bindings {
        let r = &jb.resource;
        let kind = r.get("kind").and_then(|k| k.as_str()).unwrap_or("");
        let array = iv(r, "array");
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
                (st, L::SSBO_BASE + nsb - 1, Some(Res::Sbuf { cb_slot: iv(r, "cb_slot") as i32, cb_off: 4 * iv(r, "cb_offset") as u32, written: iv(r, "flags") & 1 != 0 }))
            }
            (_, "sampled_image") | (_, "sampler") | (_, "storage_image") | (_, "uniform_texel_buffer") | (_, "storage_texel_buffer")
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
                    return Err(format!("texture array of {array} (handle tables) is not bound yet"));
                }
                if ntex >= L::TEX_N {
                    return Err("more than 64 textures".into());
                }
                ntex += 1;
                (st, L::TEX_BASE + ntex - 1, Some(Res::Tex { handle: iv(r, "handle") as u32, cbuf_slot: iv(r, "cbuf_slot") as i32, ty: iv(r, "type") as u32 }))
            }
            ("texture", "uniform_texel_buffer") => {
                ntb += 1;
                (st, L::TBUF_BASE + ntb - 1, Some(Res::Tex { handle: iv(r, "handle") as u32, cbuf_slot: iv(r, "cbuf_slot") as i32, ty: 2 }))
            }
            ("image", "storage_image") => {
                nimg += 1;
                (st, L::IMG_BASE + nimg - 1, Some(Res::Img { handle: iv(r, "handle") as u32, cbuf_slot: iv(r, "cbuf_slot") as i32, ty: iv(r, "type") as u32 }))
            }
            ("image", "storage_texel_buffer") => {
                nib += 1;
                (st, L::IBUF_BASE + nib - 1, Some(Res::Img { handle: iv(r, "handle") as u32, cbuf_slot: iv(r, "cbuf_slot") as i32, ty: 2 }))
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
    for &(id, s, b) in &refl.vars {
        match map.get(&(s, b)) {
            Some(&(ns, nb)) => redecorate(&mut w, id, ns, nb),
            None => return Err(format!("variable at set {s} binding {b} is not in the reflection")),
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
        let dirs: Vec<PathBuf> = dir.iter().flat_map(|d| [d.join("spv"), d.clone()]).chain(super::live::spv_dir(key)).collect();
        for d in dirs {
            if let Ok(bytes) = std::fs::read(d.join(format!("{hex}.spv"))) {
                if bytes.len() % 4 != 0 {
                    crate::log::red(&format!("shader {hex}.spv is not whole words"));
                    return None;
                }
                let words = bytes.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
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
        if self.mem.lock().unwrap().contains_key(key) || self.modules.lock().unwrap().get(key).is_some_and(|m| m.is_some()) {
            return true;
        }
        let Some(dir) = self.dir.lock().unwrap().clone() else { return false };
        let hex = key_hex(key);
        dir.join("spv").join(format!("{hex}.spv")).is_file() || dir.join(format!("{hex}.spv")).is_file() || super::live::spv_dir(key).is_some()
    }

    /// As `get`; `force`: a missing key is looked for again now (a draw waiting for the live translator).
    /// Whether `key` was loaded and refused (its module cannot be built): waiting for it is pointless.
    pub fn refused(&self, key: &ShaderKey) -> bool {
        self.modules.lock().unwrap().get(key).is_some_and(|m| m.is_none())
    }

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
                crate::log::red_once(&format!("shader {} is not in the shader library", key_hex(key)));
                self.missing.lock().unwrap().insert(*key, std::time::Instant::now());
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
    pub fn per_primitive(&self, dev: &ash::Device, key: &ShaderKey, locs: &[u32]) -> Option<Arc<Module>> {
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
                    let w2: Vec<u32> = b.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
                    match self.build(dev, &vk, &w2, j.as_deref()) {
                        Ok(m) => Some(m),
                        Err(e) => {
                            crate::log::red_once(&format!("pixel shader {}: per-primitive variant: {e}", key_hex(key)));
                            None
                        }
                    }
                }
                Err(e) => {
                    crate::log::red_once(&format!("pixel shader {}: no per-primitive variant ({e}): drawn as built", key_hex(key)));
                    None
                }
            }
        });
        self.modules.lock().unwrap().insert(vk, built.clone());
        built
    }

    fn build(&self, dev: &ash::Device, key: &ShaderKey, words: &[u32], json: Option<&str>) -> Result<Arc<Module>, String> {
        let r = reflect(words)?;
        let (w, res, pool, c1) = remap(words, &r, json)?;
        let jr = json.and_then(|j| serde_json::from_str::<JRefl>(j).ok());
        let omap = jr.as_ref().and_then(|j| j.frag_out_map).filter(|&m| m >= 0 && r.stage == 1).map(|m| m as u32);
        let xfb = jr.as_ref().and_then(|j| j.transform_feedback.as_ref()).and_then(super::xfb::Layout::from_json);
        let ci = vk::ShaderModuleCreateInfo::default().code(&w);
        let module = unsafe { dev.create_shader_module(&ci, None) }.map_err(|e| format!("vkCreateShaderModule: {e}"))?;
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
            per_primitive: jr.as_ref().map(|j| j.per_primitive_locations.clone()).unwrap_or_default(),
            id: self.next_id.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        }))
    }
}

impl Default for Library {
    fn default() -> Self {
        Self::new()
    }
}
