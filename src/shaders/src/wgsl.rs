//! SPIR-V -> WGSL for the web build (docs/35 §4.4, wave WEB-W4).
//!
//! The web device (`graphics::web`) receives the replay's final SPIR-V (the shared layout's sets and bindings,
//! `graphics::vk::layout`) at `vkCreateShaderModule` and turns it into WGSL here. Two stages:
//!
//! 1. SPIR-V rewrites (W0(e), `build/WEB/W0-shaders/spvweb.py`): the unused fragment-shader interlock goes; combined
//!    image-samplers are split into a texture (binding b) and a sampler (binding b + `SAMPLER_SHIFT`); gl_PerVertex
//!    members WebGPU has no builtin for (PointSize, ClipDistance, CullDistance) move to a private dummy; OpIsNan /
//!    OpIsInf become exact tests on the binary32 bit pattern.
//! 2. naga IR changes: descriptor sets become bind groups (`group_of`); push constants become a uniform at group 3
//!    binding 0; a per-draw uniform at group 3 binding 1 (`DRAW_*`) carries what WebGPU has no state for: the
//!    viewport's Y direction (Vulkan's negative-height viewports), depth clip "negative one to one", and the base
//!    instance / base vertex builtins; vertex-stage storage buffers are read only. The WGSL starts with
//!    `diagnostic(off, derivative_uniformity);` (Vulkan allows implicit derivatives in divergent flow).

use naga::{
    AddressSpace, BinaryOperator, Binding, BuiltIn, Expression, Handle, ImageClass, ImageDimension,
    ResourceBinding, Scalar, ScalarKind, ShaderStage, Span, Statement, StorageAccess, StructMember,
    Type, TypeInner,
};

/// A split combined image-sampler's sampler is at its texture's binding plus this.
pub const SAMPLER_SHIFT: u32 = 512;
/// The bind group of the push constants (binding 0) and the per-draw uniform (binding 1).
pub const UNIFORM_GROUP: u32 = 3;
/// The per-draw uniform: f32 y scale (+1 / -1), f32 z mode (1: map [-w, w] to [0, w]), u32 base instance, u32 base
/// vertex (16 bytes).
pub const DRAW_SIZE: u32 = 16;

/// The bind group a descriptor set goes to: a stage set (0 vertex, 1 fragment, 5 compute) or the pool set (6) of the
/// shared layout; a module of a layout of its own (sets 0-2) keeps its set.
pub fn group_of(set: u32) -> Option<u32> {
    match set {
        0 | 1 | 2 => Some(set),
        5 => Some(0),
        6 => Some(2),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Stage {
    Vertex,
    Fragment,
    Compute,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// a uniform buffer of at least `min` bytes
    Uniform { min: u32 },
    Storage { read_only: bool },
    /// sample: "float", "unfilterable-float", "depth", "sint", "uint"; dim: "1d", "2d", "2d-array", "cube",
    /// "cube-array", "3d"
    Texture { sample: &'static str, dim: &'static str, multi: bool },
    StorageTexture { format: &'static str, access: &'static str, dim: &'static str },
    Sampler { comparison: bool },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Bind {
    pub group: u32,
    pub binding: u32,
    /// the descriptor set and binding it came from (a split sampler: its texture's binding)
    pub set: u32,
    pub src_binding: u32,
    pub kind: Kind,
}

#[derive(Clone, Debug)]
pub struct Wgsl {
    pub code: String,
    pub stage: Stage,
    pub entry: String,
    pub binds: Vec<Bind>,
    /// the module reads the push constants (group 3 binding 0) as a uniform of this many bytes
    pub push: Option<u32>,
    pub workgroup: [u32; 3],
    /// vertex inputs by location: the scalar kind ('f', 'i', 'u') and component count
    pub inputs: Vec<(u32, char, u32)>,
    /// fragment outputs by location (colour targets written)
    pub outputs: Vec<u32>,
    /// the interpolation of the stage interface by location: a vertex module's outputs, a fragment module's inputs
    pub interp: Vec<(u32, Interp)>,
    /// what this conversion changed from the module's own interface (`Variant::default()`: nothing)
    pub variant: Variant,
    /// the module's SPIR-V, for a variant made later (`Wgsl::variant`)
    pub spirv: std::sync::Arc<Vec<u32>>,
}

/// An interface location's interpolation and sampling.
pub type Interp = (Option<naga::Interpolation>, Option<naga::Sampling>);

/// A vertex attribute format WebGPU has none for, fed as one `uint32` and unpacked in the vertex stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Packed {
    /// A2B10G10R10: x in bits 0-9, y 10-19, z 20-29, w 30-31; `signed` two's complement fields; `norm` normalized
    /// (SNORM: v / (2^(n-1) - 1) clamped to -1, UNORM: v / (2^n - 1)), else the integer value (UINT/SINT, or its
    /// float for USCALED/SSCALED).
    A2b10g10r10 { signed: bool, norm: bool },
}

/// A conversion of a module that differs from its own interface, made per pipeline (wave WEB-W10b).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Variant {
    /// vertex outputs by location take this interpolation (Vulkan uses the fragment stage's; WebGPU needs both equal)
    pub interp: Vec<(u32, Interp)>,
    /// vertex inputs by location fed packed (`Packed`)
    pub packed: Vec<(u32, Packed)>,
}

impl Wgsl {
    /// This module converted again with `v` applied.
    pub fn variant(&self, v: &Variant) -> Result<Wgsl, String> {
        convert_with(&self.spirv, v)
    }
}

/// The outputs of vertex module `vs` whose interpolation differs from fragment module `fs`'s inputs at the same
/// location, with the fragment's (None: they agree).
pub fn interp_fix(vs: &Wgsl, fs: &Wgsl) -> Option<Vec<(u32, Interp)>> {
    let fix: Vec<(u32, Interp)> = vs
        .interp
        .iter()
        .filter_map(|&(loc, i)| fs.interp.iter().find(|f| f.0 == loc).filter(|f| f.1 != i).copied())
        .collect();
    (!fix.is_empty()).then_some(fix)
}

/// Converts one module. The error names what the web device cannot express.
pub fn convert(words: &[u32]) -> Result<Wgsl, String> {
    convert_with(words, &Variant::default())
}

/// Converts one module with a variant's changes (`Variant`).
pub fn convert_with(spirv: &[u32], variant: &Variant) -> Result<Wgsl, String> {
    let words = rewrite_spirv(spirv)?;
    let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let opts = naga::front::spv::Options {
        adjust_coordinate_space: true,
        strict_capabilities: false,
        block_ctx_dump_prefix: None,
    };
    let mut m = naga::front::spv::parse_u8_slice(&bytes, &opts).map_err(|e| format!("naga spv-in: {e}"))?;
    if m.entry_points.len() != 1 {
        return Err(format!("{} entry points", m.entry_points.len()));
    }
    let draw = add_draw_uniform(&mut m);
    lower_position(&mut m, draw)?;
    lower_draw_builtins(&mut m, draw)?;
    let push = retarget_globals(&mut m)?;
    set_interp(&mut m, &variant.interp);
    unpack_inputs(&mut m, &variant.packed)?;
    let mut v = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all());
    let info = v.validate(&m).map_err(|e| format!("naga validation: {}", e.emit_to_string("")))?;
    let code = naga::back::wgsl::write_string(&m, &info, naga::back::wgsl::WriterFlags::empty())
        .map_err(|e| format!("naga wgsl-out: {e}"))?;
    let ep = &m.entry_points[0];
    let stage = match ep.stage {
        ShaderStage::Vertex => Stage::Vertex,
        ShaderStage::Fragment => Stage::Fragment,
        ShaderStage::Compute => Stage::Compute,
        s => return Err(format!("{s:?} stage: WebGPU has none")),
    };
    let binds = reflect(&m)?;
    let mut inputs = Vec::new();
    let mut outputs = Vec::new();
    for a in &ep.function.arguments {
        io_locations(&m, a.ty, a.binding.as_ref(), &mut inputs);
    }
    if let Some(r) = &ep.function.result {
        let mut outs = Vec::new();
        io_locations(&m, r.ty, r.binding.as_ref(), &mut outs);
        outputs = outs.into_iter().map(|o| o.0).collect();
    }
    let mut interp = Vec::new();
    match stage {
        Stage::Vertex => {
            if let Some(r) = &ep.function.result {
                interp_locations(&m, r.ty, r.binding.as_ref(), &mut interp);
            }
        }
        Stage::Fragment => {
            for a in &ep.function.arguments {
                interp_locations(&m, a.ty, a.binding.as_ref(), &mut interp);
            }
        }
        Stage::Compute => {}
    }
    Ok(Wgsl {
        interp,
        variant: variant.clone(),
        spirv: std::sync::Arc::new(spirv.to_vec()),
        code: format!("diagnostic(off, derivative_uniformity);\n{code}"),
        stage,
        entry: ep.name.clone(),
        binds,
        push,
        workgroup: ep.workgroup_size,
        inputs,
        outputs,
    })
}

fn io_locations(m: &naga::Module, ty: Handle<Type>, b: Option<&Binding>, out: &mut Vec<(u32, char, u32)>) {
    match (b, &m.types[ty].inner) {
        (Some(Binding::Location { location, .. }), inner) => {
            let (k, n) = match *inner {
                TypeInner::Scalar(s) => (s.kind, 1),
                TypeInner::Vector { size, scalar } => (scalar.kind, size as u32),
                _ => (ScalarKind::Float, 4),
            };
            let c = match k {
                ScalarKind::Sint => 'i',
                ScalarKind::Uint => 'u',
                _ => 'f',
            };
            out.push((*location, c, n));
        }
        (None, TypeInner::Struct { members, .. }) => {
            for mem in members {
                io_locations(m, mem.ty, mem.binding.as_ref(), out);
            }
        }
        _ => {}
    }
}

fn interp_locations(m: &naga::Module, ty: Handle<Type>, b: Option<&Binding>, out: &mut Vec<(u32, Interp)>) {
    match (b, &m.types[ty].inner) {
        (Some(Binding::Location { location, interpolation, sampling, .. }), _) => {
            out.push((*location, (*interpolation, *sampling)));
        }
        (None, TypeInner::Struct { members, .. }) => {
            for mem in members {
                interp_locations(m, mem.ty, mem.binding.as_ref(), out);
            }
        }
        _ => {}
    }
}

/// A vertex module's outputs at the given locations take the given interpolation (`Variant::interp`). The entry's
/// result struct is replaced by a copy with those bindings (types are unique: not changed in place), and the entry's
/// compositions of it take the copy.
fn set_interp(m: &mut naga::Module, want: &[(u32, Interp)]) {
    let set = |b: &mut Option<Binding>| {
        if let Some(Binding::Location { location, interpolation, sampling, .. }) = b {
            if let Some(&(_, (i, s))) = want.iter().find(|w| w.0 == *location) {
                *interpolation = i;
                *sampling = s;
            }
        }
    };
    if want.is_empty() || m.entry_points[0].stage != ShaderStage::Vertex {
        return;
    }
    let Some(r) = m.entry_points[0].function.result.as_mut() else { return };
    if r.binding.is_some() {
        set(&mut r.binding);
        return;
    }
    let old = r.ty;
    let TypeInner::Struct { mut members, span } = m.types[old].inner.clone() else { return };
    for mem in &mut members {
        set(&mut mem.binding);
    }
    let name = m.types[old].name.clone().map(|n| format!("{n}_nxi"));
    let new = m.types.insert(Type { name, inner: TypeInner::Struct { members, span } }, Span::UNDEFINED);
    let f = &mut m.entry_points[0].function;
    if let Some(r) = f.result.as_mut() {
        r.ty = new;
    }
    for (_, e) in f.expressions.iter_mut() {
        match e {
            Expression::Compose { ty, .. } | Expression::ZeroValue(ty) if *ty == old => *ty = new,
            _ => {}
        }
    }
    for (_, l) in f.local_variables.iter_mut() {
        if l.ty == old {
            l.ty = new;
        }
    }
}

/// Vertex inputs fed packed (`Variant::packed`): the entry's argument becomes a `u32`, and the value it stored into
/// the module's input is unpacked from it (bit fields, then the shader's scalar kind, normalized when the format is).
fn unpack_inputs(m: &mut naga::Module, packed: &[(u32, Packed)]) -> Result<(), String> {
    if packed.is_empty() {
        return Ok(());
    }
    let u = u32_ty(m);
    for &(loc, p) in packed {
        let Packed::A2b10g10r10 { signed, norm } = p;
        let Some(i) = m.entry_points[0].function.arguments.iter().position(
            |a| matches!(a.binding, Some(Binding::Location { location, .. }) if location == loc),
        ) else {
            continue;
        };
        let old_ty = m.entry_points[0].function.arguments[i].ty;
        let (n, kind) = match m.types[old_ty].inner {
            TypeInner::Scalar(s) => (1u32, s.kind),
            TypeInner::Vector { size, scalar } => (size as u32, scalar.kind),
            _ => return Err(format!("packed vertex input {loc}: not a scalar or vector")),
        };
        let f = &mut m.entry_points[0].function;
        f.arguments[i].ty = u;
        if let Some(Binding::Location { interpolation, sampling, .. }) = &mut f.arguments[i].binding {
            *interpolation = None;
            *sampling = None;
        }
        let uses: Vec<Handle<Expression>> = f
            .expressions
            .iter()
            .filter(|(_, e)| matches!(e, Expression::FunctionArgument(a) if *a as usize == i))
            .map(|(h, _)| h)
            .collect();
        let Some(&arg) = uses.first() else { continue };
        let ex = &mut f.expressions;
        let mut emits = Vec::new();
        let mut add = |e: Expression| {
            let lit = matches!(e, Expression::Literal(_));
            let h = ex.append(e, Span::UNDEFINED);
            if !lit {
                emits.push(Statement::Emit(naga::Range::new_from_bounds(h, h)));
            }
            h
        };
        let lu = |v: u32| Expression::Literal(naga::Literal::U32(v));
        let bin = |op, left, right| Expression::Binary { op, left, right };
        let bits = if signed { add(Expression::As { expr: arg, kind: ScalarKind::Sint, convert: None }) } else { arg };
        let mut comps = Vec::new();
        for (off, width) in [(0u32, 10u32), (10, 10), (20, 10), (30, 2)].into_iter().take(n as usize) {
            // the field as an integer of the packed signedness
            let v = if signed {
                let l = add(lu(32 - off - width));
                let up = add(bin(BinaryOperator::ShiftLeft, bits, l));
                let r = add(lu(32 - width));
                add(bin(BinaryOperator::ShiftRight, up, r))
            } else {
                let o = add(lu(off));
                let down = add(bin(BinaryOperator::ShiftRight, bits, o));
                let mask = add(lu((1 << width) - 1));
                add(bin(BinaryOperator::And, down, mask))
            };
            let c = match kind {
                ScalarKind::Float => {
                    let fv = add(Expression::As { expr: v, kind: ScalarKind::Float, convert: Some(4) });
                    if norm {
                        let max = if signed { (1u32 << (width - 1)) - 1 } else { (1u32 << width) - 1 };
                        let s = add(Expression::Literal(naga::Literal::F32(1.0 / max as f32)));
                        let scaled = add(bin(BinaryOperator::Multiply, fv, s));
                        if signed {
                            let lo = add(Expression::Literal(naga::Literal::F32(-1.0)));
                            add(Expression::Math {
                                fun: naga::MathFunction::Max,
                                arg: scaled,
                                arg1: Some(lo),
                                arg2: None,
                                arg3: None,
                            })
                        } else {
                            scaled
                        }
                    } else {
                        fv
                    }
                }
                ScalarKind::Sint if !signed => add(Expression::As { expr: v, kind: ScalarKind::Sint, convert: None }),
                ScalarKind::Uint if signed => add(Expression::As { expr: v, kind: ScalarKind::Uint, convert: None }),
                _ => v,
            };
            comps.push(c);
        }
        let value = if n == 1 { comps[0] } else { add(Expression::Compose { ty: old_ty, components: comps }) };
        let mut body = naga::Block::new();
        for s in emits {
            body.push(s, Span::UNDEFINED);
        }
        let old = std::mem::take(&mut f.body);
        let mut stored = 0;
        for s in old.iter().cloned() {
            let s = match s {
                Statement::Store { pointer, value: v } if uses.contains(&v) => {
                    stored += 1;
                    Statement::Store { pointer, value }
                }
                s => s,
            };
            body.push(s, Span::UNDEFINED);
        }
        if stored == 0 {
            return Err(format!("packed vertex input {loc}: read other than through the entry's store"));
        }
        f.body = body;
    }
    Ok(())
}

// ---------- 1. SPIR-V rewrites ----------

fn op(x: &[u32]) -> u32 {
    x[0] & 0xffff
}
fn mk(o: u32, a: &[u32]) -> Vec<u32> {
    let mut v = Vec::with_capacity(a.len() + 1);
    v.push(((a.len() as u32 + 1) << 16) | o);
    v.extend_from_slice(a);
    v
}

struct Ids {
    bound: u32,
}
impl Ids {
    fn fresh(&mut self) -> u32 {
        self.bound += 1;
        self.bound - 1
    }
}

/// The W0(e) rewrites on the module's words (see the module doc).
pub fn rewrite_spirv(w: &[u32]) -> Result<Vec<u32>, String> {
    if w.len() < 5 || w[0] != 0x0723_0203 {
        return Err("not SPIR-V".into());
    }
    let mut hdr = w[..5].to_vec();
    let mut ids = Ids { bound: w[3] };
    let mut ins: Vec<Vec<u32>> = Vec::new();
    let mut i = 5;
    while i < w.len() {
        let n = (w[i] >> 16) as usize;
        if n == 0 || i + n > w.len() {
            return Err("truncated instruction".into());
        }
        ins.push(w[i..i + n].to_vec());
        i += n;
    }
    // 1. interlock: declared, unused
    if ins.iter().any(|x| matches!(op(x), 5364 | 5365)) {
        return Err("fragment shader interlock is used (WebGPU has none)".into());
    }
    ins.retain(|x| {
        let o = op(x);
        !((o == 17 && matches!(x[1], 5378..=5380))
            || (o == 10 && words_str(&x[1..]).contains("interlock"))
            || (o == 16 && x.len() > 2 && (5366..=5371).contains(&x[2])))
    });
    use std::collections::{HashMap, HashSet};
    let types: HashMap<u32, Vec<u32>> = ins
        .iter()
        .filter(|x| matches!(op(x), 19..=30 | 32))
        .map(|x| (x[1], x.clone()))
        .collect();
    let consts: HashMap<u32, u32> = ins.iter().filter(|x| op(x) == 43).map(|x| (x[2], x[3])).collect();
    let mut ptr_of: HashMap<(u32, u32), u32> =
        ins.iter().filter(|x| op(x) == 32).map(|x| ((x[2], x[3]), x[1])).collect();
    let first_fn = ins.iter().position(|x| op(x) == 54).ok_or("no function")?;
    let mut new_types: Vec<Vec<u32>> = Vec::new();
    let mut new_vars: Vec<Vec<u32>> = Vec::new();
    let mut deco: Vec<Vec<u32>> = Vec::new();
    let mut pointer = |ids: &mut Ids, nt: &mut Vec<Vec<u32>>, storage: u32, pointee: u32| -> u32 {
        *ptr_of.entry((storage, pointee)).or_insert_with(|| {
            let p = ids.fresh();
            nt.push(mk(32, &[p, storage, pointee]));
            p
        })
    };
    let sampler = match ins.iter().find(|x| op(x) == 26) {
        Some(x) => x[1],
        None => {
            let s = ids.fresh();
            new_types.push(mk(26, &[s]));
            s
        }
    };
    // 2. combined image-samplers: pointer (UniformConstant) -> sampled image type
    let sampled_ptr: HashMap<u32, u32> = ins
        .iter()
        .filter(|x| op(x) == 32 && x[2] == 0 && types.get(&x[3]).is_some_and(|t| op(t) == 27))
        .map(|x| (x[1], x[3]))
        .collect();
    // arrays of combined samplers (the pool set's bindless arrays) have no WebGPU form
    for x in &ins {
        if op(x) == 32 && x[2] == 0 {
            if let Some(t) = types.get(&x[3]) {
                if matches!(op(t), 28 | 29) && types.get(&t[2]).is_some_and(|e| op(e) == 27) {
                    if ins.iter().any(|v| op(v) == 59 && v[1] == x[1]) {
                        return Err("an array of combined image-samplers (WebGPU has no binding arrays)".into());
                    }
                }
            }
        }
    }
    let combined: HashMap<u32, u32> = ins
        .iter()
        .filter(|x| op(x) == 59 && sampled_ptr.contains_key(&x[1]))
        .map(|x| (x[2], x[1]))
        .collect();
    let mut split: HashMap<u32, (u32, u32, u32, u32)> = HashMap::new();
    for (&v, p) in &combined {
        let si = sampled_ptr[p];
        let img = types[&si][2];
        let (vi, vs) = (ids.fresh(), ids.fresh());
        let pi = pointer(&mut ids, &mut new_types, 0, img);
        let ps = pointer(&mut ids, &mut new_types, 0, sampler);
        new_vars.push(mk(59, &[pi, vi, 0]));
        new_vars.push(mk(59, &[ps, vs, 0]));
        split.insert(v, (vi, vs, img, si));
    }
    for x in &ins {
        if op(x) == 71 && split.contains_key(&x[1]) && matches!(x[2], 33 | 34) {
            let (vi, vs, ..) = split[&x[1]];
            deco.push(mk(71, &[vi, x[2], x[3]]));
            let b = if x[2] == 33 { x[3] + SAMPLER_SHIFT } else { x[3] };
            deco.push(mk(71, &[vs, x[2], b]));
        }
    }
    // 3. gl_PerVertex members without a WebGPU builtin
    let mut drop: HashMap<u32, HashSet<u32>> = HashMap::new();
    for x in &ins {
        if op(x) == 72 && x.len() > 4 && x[3] == 11 && matches!(x[4], 1 | 3 | 4) {
            drop.entry(x[1]).or_default().insert(x[2]);
        }
    }
    let mut pv: HashMap<u32, (u32, HashSet<u32>, u32)> = HashMap::new();
    for (&st, members) in &drop {
        let all = types.get(&st).ok_or("gl_PerVertex type")?[2..].to_vec();
        let keep: Vec<u32> = (0..all.len() as u32).filter(|m| !members.contains(m)).collect();
        if keep.iter().enumerate().any(|(k, &m)| k as u32 != m) {
            return Err("a dropped builtin member before a kept one".into());
        }
        let st2 = ids.fresh();
        let mut t = vec![st2];
        t.extend(keep.iter().map(|&m| all[m as usize]));
        new_types.push(mk(30, &t));
        for x in &ins {
            if op(x) == 72 && x[1] == st && keep.contains(&x[2]) {
                let mut d = vec![st2];
                d.extend_from_slice(&x[2..]);
                deco.push(mk(72, &d));
            }
            if op(x) == 71 && x[1] == st {
                let mut d = vec![st2];
                d.extend_from_slice(&x[2..]);
                deco.push(mk(71, &d));
            }
        }
        for x in &ins {
            if op(x) == 59 && x[3] == 3 && types.get(&x[1]).is_some_and(|t| t.len() > 3 && t[3] == st) {
                let d = ids.fresh();
                let pp = pointer(&mut ids, &mut new_types, 6, st);
                new_vars.push(mk(59, &[pp, d, 6]));
                pv.insert(x[2], (d, keep.iter().copied().collect(), st2));
            }
        }
    }
    // 4. isnan / isinf helpers
    let mut uints = Uints::default();
    // rewrite
    let mut out: Vec<Vec<u32>> = Vec::with_capacity(ins.len() + 16);
    let mut mark = None;
    for (k, x) in ins.iter().enumerate() {
        let o = op(x);
        if k == first_fn {
            mark = Some(out.len());
        }
        if o == 59 && combined.contains_key(&x[2]) {
            continue;
        }
        if matches!(o, 5 | 71) && combined.contains_key(&x[1]) {
            continue;
        }
        if o == 15 && x[4..].iter().any(|v| split.contains_key(v)) {
            // OpEntryPoint: SPIR-V 1.4+ lists every global the entry point uses (glslang's own shaders, e.g. the
            // present's upscale); a removed combined variable is replaced by its two halves (WEB-W8: naga refused the
            // dangling id). The name ends at the first word with a zero byte.
            let name_end = 3 + x[3..].iter().position(|w| w.to_le_bytes().contains(&0)).ok_or("entry point name")? + 1;
            let mut e = x[..name_end].to_vec();
            for v in &x[name_end..] {
                match split.get(v) {
                    Some(&(vi, vs, ..)) => e.extend([vi, vs]),
                    None => e.push(*v),
                }
            }
            e[0] = ((e.len() as u32) << 16) | 15;
            out.push(e);
            continue;
        }
        if o == 59 && pv.contains_key(&x[2]) {
            let pp = pointer(&mut ids, &mut new_types, 3, pv[&x[2]].2);
            new_vars.push(mk(59, &[pp, x[2], 3]));
            continue;
        }
        if o == 61 && split.contains_key(&x[3]) {
            let (vi, vs, img, si) = split[&x[3]];
            let (a, b) = (ids.fresh(), ids.fresh());
            out.push(mk(61, &[img, a, vi]));
            out.push(mk(61, &[sampler, b, vs]));
            out.push(mk(86, &[si, x[2], a, b]));
            continue;
        }
        if matches!(o, 65 | 66) && pv.contains_key(&x[3]) {
            let (d, keep, _) = &pv[&x[3]];
            if !consts.get(&x[4]).is_some_and(|c| keep.contains(c)) {
                let pointee = types.get(&x[1]).ok_or("access chain type")?[3];
                let pp = pointer(&mut ids, &mut new_types, 6, pointee);
                let mut a = vec![pp, x[2], *d];
                a.extend_from_slice(&x[4..]);
                out.push(mk(o, &a));
                continue;
            }
        }
        if matches!(o, 61..=63) && pv.contains_key(&(if o == 61 { x[3] } else { x[1] })) {
            return Err("a whole-block load or store of gl_PerVertex".into());
        }
        if matches!(o, 71 | 72) && drop.contains_key(&x[1]) {
            continue;
        }
        if o == 147 {
            // OpOuterProduct (WGSL has no outer product): column j = a * b[j]
            let (mt, res, a, b) = (x[1], x[2], x[3], x[4]);
            let m = types.get(&mt).ok_or("outer product type")?;
            let (col_ty, cols) = (m[2], m[3]);
            let scalar = types.get(&col_ty).ok_or("outer product column type")?[2];
            let mut parts = vec![mt, res];
            for j in 0..cols {
                let (e, c) = (ids.fresh(), ids.fresh());
                out.push(mk(81, &[scalar, e, b, j]));
                out.push(mk(142, &[col_ty, c, a, e]));
                parts.push(c);
            }
            out.push(mk(80, &parts));
            continue;
        }
        if matches!(o, 156 | 157) {
            let rt = x[1];
            let n = types.get(&rt).map_or(1, |t| if op(t) == 23 { t[3] } else { 1 });
            let ut = uints.ty(&mut ids, &mut new_types, n, &ins);
            let (b, a) = (ids.fresh(), ids.fresh());
            let mask = uints.constant(&mut ids, &mut new_types, 0x7fff_ffff, n, &ins);
            let inf = uints.constant(&mut ids, &mut new_types, 0x7f80_0000, n, &ins);
            let cmp = if o == 156 { 172 } else { 170 };
            out.push(mk(124, &[ut, b, x[3]]));
            out.push(mk(199, &[ut, a, b, mask]));
            out.push(mk(cmp, &[rt, x[2], a, inf]));
            continue;
        }
        out.push(x.clone());
    }
    let m = mark.ok_or("no function")?;
    // new types and variables before the first function, after the module's own globals: a new pointer's pointee
    // may be any existing type
    let mut tail = out.split_off(m);
    out.extend(new_types);
    out.extend(new_vars);
    // decorations go after the last existing one (annotations precede types)
    let ld = out.iter().rposition(|x| matches!(op(x), 71 | 72)).ok_or("no decorations")?;
    let after = out.split_off(ld + 1);
    out.extend(deco);
    out.extend(after);
    out.append(&mut tail);
    hdr[3] = ids.bound;
    let mut flat = hdr;
    for x in out {
        flat.extend(x);
    }
    Ok(flat)
}

/// 32-bit unsigned types and constants the isnan / isinf lowering needs, found or made once.
#[derive(Default)]
struct Uints {
    ty: std::collections::HashMap<u32, u32>,
    consts: std::collections::HashMap<(u32, u32), u32>,
}

impl Uints {
    fn ty(&mut self, ids: &mut Ids, nt: &mut Vec<Vec<u32>>, n: u32, ins: &[Vec<u32>]) -> u32 {
        if let Some(&t) = self.ty.get(&n) {
            return t;
        }
        let t = if n == 1 {
            ins.iter().chain(nt.iter()).find(|x| op(x) == 21 && x[2] == 32 && x[3] == 0).map(|x| x[1])
        } else {
            let s = self.ty(ids, nt, 1, ins);
            ins.iter().chain(nt.iter()).find(|x| op(x) == 23 && x[2] == s && x[3] == n).map(|x| x[1])
        };
        let t = match t {
            Some(t) => t,
            None => {
                let t = ids.fresh();
                if n == 1 {
                    nt.push(mk(21, &[t, 32, 0]));
                } else {
                    let s = self.ty(ids, nt, 1, ins);
                    nt.push(mk(23, &[t, s, n]));
                }
                t
            }
        };
        self.ty.insert(n, t);
        t
    }

    fn constant(&mut self, ids: &mut Ids, nt: &mut Vec<Vec<u32>>, v: u32, n: u32, ins: &[Vec<u32>]) -> u32 {
        if let Some(&c) = self.consts.get(&(v, n)) {
            return c;
        }
        let c = if n == 1 {
            let s = self.ty(ids, nt, 1, ins);
            let c = ids.fresh();
            nt.push(mk(43, &[s, c, v]));
            c
        } else {
            let one = self.constant(ids, nt, v, 1, ins);
            let vt = self.ty(ids, nt, n, ins);
            let c = ids.fresh();
            let mut a = vec![vt, c];
            a.extend(std::iter::repeat_n(one, n as usize));
            nt.push(mk(44, &a));
            c
        };
        self.consts.insert((v, n), c);
        c
    }
}

fn words_str(w: &[u32]) -> String {
    let b: Vec<u8> = w.iter().flat_map(|x| x.to_le_bytes()).take_while(|&c| c != 0).collect();
    String::from_utf8_lossy(&b).into_owned()
}

// ---------- 2. naga IR ----------

fn u32_ty(m: &mut naga::Module) -> Handle<Type> {
    m.types.insert(Type { name: None, inner: TypeInner::Scalar(Scalar { kind: ScalarKind::Uint, width: 4 }) }, Span::UNDEFINED)
}
fn f32_ty(m: &mut naga::Module) -> Handle<Type> {
    m.types.insert(Type { name: None, inner: TypeInner::Scalar(Scalar { kind: ScalarKind::Float, width: 4 }) }, Span::UNDEFINED)
}

/// The per-draw uniform (group 3 binding 1), added to every vertex module (unused ones are dropped by naga's writer
/// only if unreferenced: it is referenced only where lowered).
fn add_draw_uniform(m: &mut naga::Module) -> Option<Handle<naga::GlobalVariable>> {
    if m.entry_points.first()?.stage != ShaderStage::Vertex {
        return None;
    }
    let f = f32_ty(m);
    let u = u32_ty(m);
    let mem = |name: &str, ty, offset| StructMember { name: Some(name.into()), ty, binding: None, offset };
    let st = m.types.insert(
        Type {
            name: Some("NxDraw".into()),
            inner: TypeInner::Struct {
                members: vec![mem("y_scale", f, 0), mem("z_mode", f, 4), mem("base_instance", u, 8), mem("base_vertex", u, 12)],
                span: DRAW_SIZE,
            },
        },
        Span::UNDEFINED,
    );
    Some(m.global_variables.append(
        naga::GlobalVariable {
            name: Some("nx_draw".into()),
            space: AddressSpace::Uniform,
            binding: Some(ResourceBinding { group: UNIFORM_GROUP, binding: 1 }),
            ty: st,
            init: None,
            memory_decorations: Default::default(),
        },
        Span::UNDEFINED,
    ))
}

/// naga's Y flip of the position (`adjust_coordinate_space`: `p.y = -p.y`) becomes `p.y = p.y * nx_draw.y_scale`, and
/// `p.z = p.z + z_mode * ((p.z + p.w) / 2 - p.z)` follows it (depth clip "negative one to one").
fn lower_position(m: &mut naga::Module, draw: Option<Handle<naga::GlobalVariable>>) -> Result<(), String> {
    let Some(draw) = draw else { return Ok(()) };
    let f = &mut m.entry_points[0].function;
    let ex = &mut f.expressions;
    let mut at = None;
    for (k, s) in f.body.iter().enumerate() {
        if let Statement::Store { pointer, value } = *s {
            if let (Expression::AccessIndex { base, index: 1 }, Expression::Unary { op: naga::UnaryOperator::Negate, expr }) =
                (&ex[pointer], &ex[value])
            {
                if matches!(ex[*expr], Expression::Load { pointer: p } if p == pointer) {
                    at = Some((k, pointer, *base, *expr));
                }
            }
        }
    }
    let Some((k, ptr_y, base, load_y)) = at else {
        return Err("no position output to flip".into());
    };
    let len = ex.len();
    let g = ex.append(Expression::GlobalVariable(draw), Span::UNDEFINED);
    let first = ex.append(Expression::AccessIndex { base: g, index: 0 }, Span::UNDEFINED);
    let ys = ex.append(Expression::Load { pointer: first }, Span::UNDEFINED);
    let ny = ex.append(Expression::Binary { op: BinaryOperator::Multiply, left: load_y, right: ys }, Span::UNDEFINED);
    let zp = ex.append(Expression::AccessIndex { base: g, index: 1 }, Span::UNDEFINED);
    let zm = ex.append(Expression::Load { pointer: zp }, Span::UNDEFINED);
    let pz = ex.append(Expression::AccessIndex { base, index: 2 }, Span::UNDEFINED);
    let pw = ex.append(Expression::AccessIndex { base, index: 3 }, Span::UNDEFINED);
    let z = ex.append(Expression::Load { pointer: pz }, Span::UNDEFINED);
    let w = ex.append(Expression::Load { pointer: pw }, Span::UNDEFINED);
    let half = ex.append(Expression::Literal(naga::Literal::F32(0.5)), Span::UNDEFINED);
    let zw = ex.append(Expression::Binary { op: BinaryOperator::Add, left: z, right: w }, Span::UNDEFINED);
    let zh = ex.append(Expression::Binary { op: BinaryOperator::Multiply, left: zw, right: half }, Span::UNDEFINED);
    let d = ex.append(Expression::Binary { op: BinaryOperator::Subtract, left: zh, right: z }, Span::UNDEFINED);
    let md = ex.append(Expression::Binary { op: BinaryOperator::Multiply, left: zm, right: d }, Span::UNDEFINED);
    let nz = ex.append(Expression::Binary { op: BinaryOperator::Add, left: z, right: md }, Span::UNDEFINED);
    // the global and the literal are pre-emitted; the rest is emitted in two runs around them
    let emit1 = naga::Range::new_from_bounds(first, ny);
    let emit2 = naga::Range::new_from_bounds(zp, w);
    let emit3 = naga::Range::new_from_bounds(zw, nz);
    let _ = len;
    let stmts = vec![
        Statement::Emit(emit1),
        Statement::Store { pointer: ptr_y, value: ny },
        Statement::Emit(emit2),
        Statement::Emit(emit3),
        Statement::Store { pointer: pz, value: nz },
    ];
    // replace naga's store of -y (the negation stays as an unused expression)
    let mut body = naga::Block::new();
    let old = std::mem::take(&mut f.body);
    for (i, s) in old.iter().cloned().enumerate() {
        if i == k {
            for t in stmts.iter().cloned() {
                body.push(t, Span::UNDEFINED);
            }
        } else {
            body.push(s, Span::UNDEFINED);
        }
    }
    f.body = body;
    Ok(())
}

/// BaseInstance / BaseVertex entry arguments (WGSL has neither) come from the per-draw uniform.
fn lower_draw_builtins(m: &mut naga::Module, draw: Option<Handle<naga::GlobalVariable>>) -> Result<(), String> {
    let f = &mut m.entry_points[0].function;
    let which: Vec<(usize, u32)> = f
        .arguments
        .iter()
        .enumerate()
        .filter_map(|(i, a)| match a.binding {
            Some(Binding::BuiltIn(BuiltIn::BaseInstance)) => Some((i, 2)),
            Some(Binding::BuiltIn(BuiltIn::BaseVertex)) => Some((i, 3)),
            _ => None,
        })
        .collect();
    if which.is_empty() {
        return Ok(());
    }
    let draw = draw.ok_or("a draw builtin outside a vertex shader")?;
    for &(i, member) in which.iter().rev() {
        let ty = f.arguments[i].ty;
        f.arguments.remove(i);
        // FunctionArgument(i) -> zero (pre-emitted, keeps its place); later arguments move down
        let mut uses = Vec::new();
        for (h, e) in f.expressions.iter_mut() {
            if let Expression::FunctionArgument(a) = e {
                if *a as usize == i {
                    *e = Expression::ZeroValue(ty);
                    uses.push(h);
                } else if *a as usize > i {
                    *a -= 1;
                }
            }
        }
        // each store of the argument stores the uniform's member instead
        let ex = &mut f.expressions;
        let g = ex.append(Expression::GlobalVariable(draw), Span::UNDEFINED);
        let p = ex.append(Expression::AccessIndex { base: g, index: member }, Span::UNDEFINED);
        let v = ex.append(Expression::Load { pointer: p }, Span::UNDEFINED);
        let mut body = naga::Block::new();
        body.push(Statement::Emit(naga::Range::new_from_bounds(p, v)), Span::UNDEFINED);
        let old = std::mem::take(&mut f.body);
        let mut stored = 0;
        for s in old.iter().cloned() {
            let s = match s {
                Statement::Store { pointer, value } if uses.contains(&value) => {
                    stored += 1;
                    Statement::Store { pointer, value: v }
                }
                // the wrapper converts the builtin to the global's type first (`i32(base_instance)`)
                Statement::Store { pointer, value } => match ex[value] {
                    Expression::As { expr, kind, convert } if uses.contains(&expr) => {
                        let c = ex.append(Expression::As { expr: v, kind, convert }, Span::UNDEFINED);
                        body.push(Statement::Emit(naga::Range::new_from_bounds(c, c)), Span::UNDEFINED);
                        stored += 1;
                        Statement::Store { pointer, value: c }
                    }
                    _ => Statement::Store { pointer, value },
                },
                s => s,
            };
            body.push(s, Span::UNDEFINED);
        }
        if stored == 0 {
            return Err("a draw builtin read other than through the entry's store".into());
        }
        f.body = body;
    }
    Ok(())
}

/// Sets -> groups, push constants -> group 3 binding 0 (a uniform), vertex-stage storage -> read only. Returns the
/// push block's size when the module has one.
fn retarget_globals(m: &mut naga::Module) -> Result<Option<u32>, String> {
    let stage = m.entry_points[0].stage;
    let mut push = None;
    let mut seen = std::collections::HashSet::new();
    for (_, g) in m.global_variables.iter_mut() {
        if g.space == AddressSpace::Immediate {
            g.space = AddressSpace::Uniform;
            g.binding = Some(ResourceBinding { group: UNIFORM_GROUP, binding: 0 });
            push = Some(g.ty);
            continue;
        }
        if let Some(b) = g.binding.as_mut() {
            if b.group == UNIFORM_GROUP && (b.binding == 1) && g.name.as_deref() == Some("nx_draw") {
                continue;
            }
            let grp = group_of(b.group).ok_or_else(|| format!("descriptor set {} has no bind group", b.group))?;
            if !seen.insert((grp, b.binding)) {
                return Err(format!("two resources at set {} binding {}", b.group, b.binding));
            }
            // the set is kept in the name's suffix for reflection (`reflect`)
            g.name = Some(format!("s{}b{}", b.group, b.binding));
            b.group = grp;
            if let AddressSpace::Storage { access } = &mut g.space {
                if stage == ShaderStage::Vertex {
                    *access = StorageAccess::LOAD;
                } else if access.contains(StorageAccess::STORE) {
                    // a `writeonly` buffer (SPIR-V NonReadable, which glslang 15 emits): WGSL storage buffers are
                    // `read` or `read_write`, so it is bound read_write
                    *access |= StorageAccess::LOAD;
                }
            }
        }
    }
    let size = push.map(|t| m.types[t].inner.size(m.to_ctx()));
    Ok(size)
}

fn reflect(m: &naga::Module) -> Result<Vec<Bind>, String> {
    let mut out = Vec::new();
    for (_, g) in m.global_variables.iter() {
        let Some(b) = g.binding else { continue };
        if b.group == UNIFORM_GROUP {
            continue;
        }
        let (set, src_binding) = g
            .name
            .as_deref()
            .and_then(|n| n.strip_prefix('s'))
            .and_then(|n| n.split_once('b'))
            .and_then(|(s, b)| Some((s.parse().ok()?, b.parse().ok()?)))
            .ok_or("an unnamed resource")?;
        let src_binding: u32 = src_binding;
        let src_binding = if src_binding >= SAMPLER_SHIFT { src_binding - SAMPLER_SHIFT } else { src_binding };
        let inner = &m.types[g.ty].inner;
        let kind = match (g.space, inner) {
            (AddressSpace::Uniform, _) => Kind::Uniform { min: inner.size(m.to_ctx()) },
            (AddressSpace::Storage { access }, _) => Kind::Storage { read_only: !access.contains(StorageAccess::STORE) },
            (_, TypeInner::Sampler { comparison }) => Kind::Sampler { comparison: *comparison },
            (_, TypeInner::Image { dim, arrayed, class }) => {
                let d = dim_str(*dim, *arrayed)?;
                match class {
                    ImageClass::Sampled { kind, multi } => Kind::Texture {
                        sample: match kind {
                            ScalarKind::Sint => "sint",
                            ScalarKind::Uint => "uint",
                            _ => "float",
                        },
                        dim: d,
                        multi: *multi,
                    },
                    ImageClass::Depth { multi } => Kind::Texture { sample: "depth", dim: d, multi: *multi },
                    ImageClass::Storage { format, access } => Kind::StorageTexture {
                        format: storage_format(*format),
                        access: if access.contains(StorageAccess::LOAD | StorageAccess::STORE) {
                            "read-write"
                        } else if access.contains(StorageAccess::STORE) {
                            "write-only"
                        } else {
                            "read-only"
                        },
                        dim: d,
                    },
                    ImageClass::External => return Err("an external texture".into()),
                }
            }
            (_, TypeInner::BindingArray { .. }) => return Err("a binding array (WebGPU has none)".into()),
            (s, t) => return Err(format!("a resource WebGPU cannot bind: {s:?} {t:?}")),
        };
        out.push(Bind { group: b.group, binding: b.binding, set, src_binding, kind });
    }
    out.sort_by_key(|b| (b.group, b.binding));
    Ok(out)
}

fn dim_str(d: ImageDimension, arrayed: bool) -> Result<&'static str, String> {
    Ok(match (d, arrayed) {
        (ImageDimension::D1, false) => "1d",
        (ImageDimension::D2, false) => "2d",
        (ImageDimension::D2, true) => "2d-array",
        (ImageDimension::Cube, false) => "cube",
        (ImageDimension::Cube, true) => "cube-array",
        (ImageDimension::D3, false) => "3d",
        (d, a) => return Err(format!("image {d:?} arrayed={a} (WebGPU has none)")),
    })
}

fn storage_format(f: naga::StorageFormat) -> &'static str {
    use naga::StorageFormat as S;
    match f {
        S::R8Unorm => "r8unorm",
        S::R8Snorm => "r8snorm",
        S::R8Uint => "r8uint",
        S::R8Sint => "r8sint",
        S::R16Uint => "r16uint",
        S::R16Sint => "r16sint",
        S::R16Float => "r16float",
        S::Rg8Unorm => "rg8unorm",
        S::Rg8Snorm => "rg8snorm",
        S::Rg8Uint => "rg8uint",
        S::Rg8Sint => "rg8sint",
        S::R32Uint => "r32uint",
        S::R32Sint => "r32sint",
        S::R32Float => "r32float",
        S::Rg16Uint => "rg16uint",
        S::Rg16Sint => "rg16sint",
        S::Rg16Float => "rg16float",
        S::Rgba8Unorm => "rgba8unorm",
        S::Rgba8Snorm => "rgba8snorm",
        S::Rgba8Uint => "rgba8uint",
        S::Rgba8Sint => "rgba8sint",
        S::Bgra8Unorm => "bgra8unorm",
        S::Rgb10a2Uint => "rgb10a2uint",
        S::Rgb10a2Unorm => "rgb10a2unorm",
        S::Rg11b10Ufloat => "rg11b10ufloat",
        S::R64Uint => "r64uint",
        S::Rg32Uint => "rg32uint",
        S::Rg32Sint => "rg32sint",
        S::Rg32Float => "rg32float",
        S::Rgba16Uint => "rgba16uint",
        S::Rgba16Sint => "rgba16sint",
        S::Rgba16Float => "rgba16float",
        S::Rgba32Uint => "rgba32uint",
        S::Rgba32Sint => "rgba32sint",
        S::Rgba32Float => "rgba32float",
        S::R16Unorm => "r16unorm",
        S::R16Snorm => "r16snorm",
        S::Rg16Unorm => "rg16unorm",
        S::Rg16Snorm => "rg16snorm",
        S::Rgba16Unorm => "rgba16unorm",
        S::Rgba16Snorm => "rgba16snorm",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spv(b: &[u8]) -> Vec<u32> {
        b.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
    }
    const VS: &[u8] = include_bytes!("../testdata/wgsl/interp.vert.spv");
    const FS: &[u8] = include_bytes!("../testdata/wgsl/interp.frag.spv");

    /// WEB-W10b: a perspective vertex output read by a `noperspective` fragment input (MK8D's pipeline 2165) is
    /// found, and the vertex module's variant emits the fragment's interpolation; the flat integer output is left.
    #[test]
    fn vertex_outputs_take_the_fragment_interpolation() {
        let vs = convert(&spv(VS)).unwrap();
        let fs = convert(&spv(FS)).unwrap();
        let persp = vs.interp.iter().find(|i| i.0 == 0).unwrap().1;
        assert_eq!(persp.0, Some(naga::Interpolation::Perspective));
        assert_eq!(fs.interp.iter().find(|i| i.0 == 0).unwrap().1 .0, Some(naga::Interpolation::Linear));
        let fix = interp_fix(&vs, &fs).expect("a mismatch");
        assert_eq!(fix.len(), 1, "{fix:?}");
        let v = vs.variant(&Variant { interp: fix, ..Default::default() }).unwrap();
        assert!(v.code.contains("@location(0) @interpolate(linear"), "{}", v.code);
        assert!(interp_fix(&v, &fs).is_none());
        assert_eq!(v.variant.interp.len(), 1);
    }

    /// WEB-W10b: A2B10G10R10 SNORM / UINT attributes are read as one u32 and unpacked to the shader's own types.
    #[test]
    fn packed_vertex_inputs_are_unpacked_from_a_u32() {
        let vs = convert(&spv(VS)).unwrap();
        assert!(!vs.code.contains("@location(1) n: u32") && !vs.code.contains("@location(1) param_1: u32"));
        let v = vs
            .variant(&Variant {
                packed: vec![
                    (1, Packed::A2b10g10r10 { signed: true, norm: true }),
                    (2, Packed::A2b10g10r10 { signed: false, norm: false }),
                ],
                ..Default::default()
            })
            .unwrap();
        // locations 1 and 2 are u32 arguments now, location 0 is untouched
        let args = v.code.lines().find(|l| l.contains("@vertex")).map_or(String::new(), |_| v.code.clone());
        assert!(args.matches(": u32").count() >= 2, "{}", v.code);
        assert!(v.code.contains("max("), "SNORM clamps to -1: {}", v.code);
        assert!(v.code.contains("0.0019569471"), "1 / 511: {}", v.code);
        assert!(v.code.contains("1023u"), "UINT masks 10 bits: {}", v.code);
        assert_eq!(v.inputs.iter().find(|i| i.0 == 1).unwrap().1, 'u');
    }

    /// A `writeonly` storage buffer (SPIR-V NonReadable: glslang 15 emits it for the replay's deswizzle, ASTC and BC7
    /// kernels) is bound `read_write` in a compute module, as WGSL has no write-only buffers; read-only stays `read`.
    #[test]
    fn write_only_storage_buffers_become_read_write() {
        use naga::{ArraySize, EntryPoint, Function, GlobalVariable, Module, Type, TypeInner};
        let mut m = Module::default();
        let ty = |name: Option<&str>, inner| Type { name: name.map(String::from), inner };
        let u = m.types.insert(ty(None, TypeInner::Scalar(Scalar::U32)), Span::UNDEFINED);
        let arr = TypeInner::Array { base: u, size: ArraySize::Dynamic, stride: 4 };
        let arr = m.types.insert(ty(None, arr), Span::UNDEFINED);
        let members = vec![StructMember { name: Some("v".into()), ty: arr, binding: None, offset: 0 }];
        let block = m.types.insert(ty(Some("B"), TypeInner::Struct { members, span: 4 }), Span::UNDEFINED);
        for (binding, access) in [(0, StorageAccess::LOAD), (1, StorageAccess::STORE)] {
            let g = GlobalVariable {
                name: None,
                space: AddressSpace::Storage { access },
                binding: Some(ResourceBinding { group: 0, binding }),
                ty: block,
                init: None,
                memory_decorations: Default::default(),
            };
            m.global_variables.append(g, Span::UNDEFINED);
        }
        m.entry_points.push(EntryPoint {
            name: "main".into(),
            stage: ShaderStage::Compute,
            early_depth_test: None,
            workgroup_size: [1, 1, 1],
            workgroup_size_overrides: None,
            function: Function::default(),
            mesh_info: None,
            task_payload: None,
            incoming_ray_payload: None,
        });
        let mut v = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all());
        assert!(v.validate(&m).is_err(), "naga refuses a write-only storage buffer");
        retarget_globals(&mut m).unwrap();
        let info = v.validate(&m).unwrap();
        let code = naga::back::wgsl::write_string(&m, &info, naga::back::wgsl::WriterFlags::empty()).unwrap();
        assert!(code.contains("var<storage> s0b0") && code.contains("var<storage, read_write> s0b1"), "{code}");
        let kinds: Vec<_> = reflect(&m).unwrap().into_iter().map(|b| b.kind).collect();
        assert_eq!(kinds, [Kind::Storage { read_only: true }, Kind::Storage { read_only: false }]);
    }

    /// Opt-in: `NXRS_WGSL_SPV=<folder of .spv>` (and `NXRS_WGSL_KEYS=<file of keys>` to take only those): every module
    /// converts; `NXRS_WGSL_OUT=<folder>` keeps the WGSL for a browser check.
    #[test]
    fn a_folder_of_modules_converts() {
        let Ok(dir) = std::env::var("NXRS_WGSL_SPV") else { return };
        let keys: Option<Vec<String>> = std::env::var("NXRS_WGSL_KEYS")
            .ok()
            .map(|f| std::fs::read_to_string(f).unwrap().split_whitespace().map(String::from).collect());
        let out = std::env::var("NXRS_WGSL_OUT").ok();
        let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "spv"))
            .filter(|p| {
                keys.as_ref()
                    .is_none_or(|k| k.iter().any(|k| p.file_stem().is_some_and(|s| s.to_string_lossy() == *k)))
            })
            .collect();
        files.sort();
        let (mut ok, mut bad) = (0, Vec::new());
        for f in &files {
            let b = std::fs::read(f).unwrap();
            let w: Vec<u32> = b.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
            match convert(&w) {
                Ok(c) => {
                    ok += 1;
                    if let Some(o) = &out {
                        let stem = f.file_stem().unwrap().to_string_lossy();
                        std::fs::write(format!("{o}/{stem}.wgsl"), &c.code).unwrap();
                    }
                }
                Err(e) => bad.push(format!("{}: {e}", f.display())),
            }
        }
        for b in &bad {
            eprintln!("{b}");
        }
        eprintln!("{ok} of {} converted", files.len());
        assert!(bad.is_empty());
    }
}
