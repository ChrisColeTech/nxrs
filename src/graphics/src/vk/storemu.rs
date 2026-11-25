//! Storage access to formats the device cannot store (emulated storage formats).
//!
//! A game may write a texture as a storage image in a format the console stores and this GPU does not: Ocarina of
//! Time writes its 2560x1440 lighting targets as E5B9G9R9 by formatless image store, and the RTX 5090 (like most
//! GPUs) has no storage use for E5B9G9R9. Such an image is made with `EXTENDED_USAGE` and storage use, and its storage
//! views take the table's same-size view format (E5B9G9R9 -> R32_UINT). A shader stage bound to such a view runs as a
//! variant whose image variable is `r32ui` and whose `OpImageWrite` / `OpImageRead` on it pack and unpack the texel
//! (`rewrite`). Sampling keeps the image's own format. Only bindings whose bound image needs it get the variant.
//! What the texel means depends on the format the game asked for: its own (E5B9G9R9: the texel is a colour, packed
//! here) or its registered `r32ui` view of the image (the game packed the word itself and stores it as the one
//! component; the translator types every formatless image float, so the variant stores the component's bits).
//!
//! RGB9E5 follows the shared-exponent encoding (Vulkan spec "Shared Exponent Format Conversion", N = 9, B = 15,
//! Emax = 31): each of R, G, B clamped to [0, sharedexp_max = 65408], NaN taken as 0 (+Inf as the maximum);
//! exp' = max(-B-1, floor(log2(max_c))) + 1 + B; max_s = floor(max_c / 2^(exp'-B-N) + 0.5); exp = exp' + 1 when
//! max_s = 2^N; each component floor(c / 2^(exp-B-N) + 0.5). The rounding is done exactly in integers (the
//! power-of-two scale is exact in float, its one extra bit rounds half up), as Mesa's `float3_to_rgb9e5` does.

use ash::vk;
use std::collections::{HashMap, HashSet};

/// How a texel of an emulated storage format is converted to and from its storage view's words.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Emul {
    /// the texel's floats packed into the image's RGB9E5 word (a store in the image's own format)
    Rgb9e5 = 1,
    /// the texel's first component's bits as the word (the game's own `r32ui` view of the image: a formatted store
    /// of a word it packed itself; the translator types every formatless image as float)
    Bits32 = 2,
}

/// (the image's format, the storage view's format, the conversion in the shader)
const TABLE: &[(vk::Format, vk::Format, Emul)] = &[(
    vk::Format::E5B9G9R9_UFLOAT_PACK32,
    vk::Format::R32_UINT,
    Emul::Rgb9e5,
)];

/// The storage view format and conversion of a format the table emulates.
pub fn of(f: vk::Format) -> Option<(vk::Format, Emul)> {
    TABLE.iter().find(|e| e.0 == f).map(|e| (e.1, e.2))
}

/// The conversion a formatless float storage image needs when bound to a storage view of format `view` of an image
/// of format `image`, for which the game asked for format `requested` (the image's own, or its registered view's):
/// the image's own format through the emulated view packs; the game's own `r32ui` view takes the bits.
pub fn emulation(image: vk::Format, requested: vk::Format, view: vk::Format) -> Option<Emul> {
    // the requested format decides what a texel means (`image_view_kind` remaps by it too): a table format asked for
    // packs into its twin view, whatever the image's own format
    if let Some((twin, e)) = of(requested) {
        return (view == twin).then_some(e);
    }
    // the game's own uint view of an emulated image: the texel's R holds the word
    let (twin, _) = of(image)?;
    (view == twin && requested == vk::Format::R32_UINT).then_some(Emul::Bits32)
}

const SHAREDEXP_MAX: f32 = 65408.0;

/// RGB9E5 encoding of (r, g, b) (CPU reference of the shader code `rewrite` emits).
pub fn pack_rgb9e5(rgb: [f32; 3]) -> u32 {
    let c = rgb.map(|x| {
        let a = if x > 0.0 { x } else { 0.0 };
        if a < SHAREDEXP_MAX {
            a
        } else {
            SHAREDEXP_MAX
        }
    });
    let m = c[0].max(c[1]).max(c[2]);
    let biased = m.to_bits() >> 23;
    let e0 = if biased > 111 { biased - 111 } else { 0 };
    let round = |x: f32, e: u32| ((x * f32::from_bits((152 - e) << 23)) as u32 + 1) >> 1;
    let e = if round(m, e0) == 512 { e0 + 1 } else { e0 };
    let s = c.map(|x| round(x, e));
    s[0] | s[1] << 9 | s[2] << 18 | e << 27
}

/// RGB9E5 decoding.
pub fn unpack_rgb9e5(v: u32) -> [f32; 3] {
    let k = f32::from_bits(((v >> 27) + 103) << 23);
    [v & 511, v >> 9 & 511, v >> 18 & 511].map(|s| s as f32 * k)
}

// opcodes
const OP_NAME: u32 = 5;
const OP_LINE: u32 = 8;
const OP_EXT_INST: u32 = 12;
const OP_TYPE_BOOL: u32 = 20;
const OP_TYPE_INT: u32 = 21;
const OP_TYPE_FLOAT: u32 = 22;
const OP_TYPE_VECTOR: u32 = 23;
const OP_TYPE_IMAGE: u32 = 25;
const OP_TYPE_POINTER: u32 = 32;
const OP_CONSTANT: u32 = 43;
const OP_FUNCTION: u32 = 54;
const OP_VARIABLE: u32 = 59;
const OP_LOAD: u32 = 61;
const OP_DECORATE: u32 = 71;
const OP_VECTOR_SHUFFLE: u32 = 79;
const OP_COMPOSITE_CONSTRUCT: u32 = 80;
const OP_COMPOSITE_EXTRACT: u32 = 81;
const OP_COMPOSITE_INSERT: u32 = 82;
const OP_IMAGE_READ: u32 = 98;
const OP_IMAGE_WRITE: u32 = 99;
const OP_IMAGE_QUERY_SIZE_LOD: u32 = 103;
const OP_IMAGE_QUERY_SIZE: u32 = 104;
const OP_IMAGE_QUERY_LEVELS: u32 = 106;
const OP_IMAGE_QUERY_SAMPLES: u32 = 107;
const OP_CONVERT_F_TO_U: u32 = 109;
const OP_CONVERT_U_TO_F: u32 = 112;
const OP_BITCAST: u32 = 124;
const OP_IADD: u32 = 128;
const OP_ISUB: u32 = 130;
const OP_FMUL: u32 = 133;
const OP_SELECT: u32 = 169;
const OP_IEQUAL: u32 = 170;
const OP_UGREATER_THAN: u32 = 172;
const OP_FORD_LESS_THAN: u32 = 184;
const OP_FORD_GREATER_THAN: u32 = 186;
const OP_SHIFT_RIGHT_LOGICAL: u32 = 194;
const OP_SHIFT_LEFT_LOGICAL: u32 = 196;
const OP_BITWISE_OR: u32 = 197;
const OP_BITWISE_AND: u32 = 199;
const OP_LOOP_MERGE: u32 = 246;
const OP_SELECTION_MERGE: u32 = 247;
const OP_SWITCH: u32 = 251;
const DEC_BINDING: u32 = 33;
const DEC_SET: u32 = 34;
const SC_UNIFORM_CONSTANT: u32 = 0;
const FMT_R32UI: u32 = 33;
const DIM_BUFFER: u32 = 5;
const DIM_SUBPASS: u32 = 6;

fn put(o: &mut Vec<u32>, op: u32, ops: &[u32]) {
    o.push(((ops.len() as u32 + 1) << 16) | op);
    o.extend_from_slice(ops);
}

/// The ids the conversion code uses.
struct Ids {
    uint: u32,
    float: u32,
    boolean: u32,
    /// uint constants by value
    u: HashMap<u32, u32>,
    /// float constants by bits
    f: HashMap<u32, u32>,
}

const UINT_CONSTS: [u32; 11] = [0, 1, 9, 18, 23, 27, 103, 111, 152, 511, 512];
const FLOAT_CONSTS: [f32; 3] = [0.0, SHAREDEXP_MAX, 1.0];

/// Straight-line conversion code (no blocks, no calls), appended to `o` with fresh ids from `bound`.
struct Emit<'a> {
    o: &'a mut Vec<u32>,
    bound: &'a mut u32,
    ids: &'a Ids,
}

impl Emit<'_> {
    fn op(&mut self, op: u32, ty: u32, args: &[u32]) -> u32 {
        let r = *self.bound;
        *self.bound += 1;
        let mut a = vec![ty, r];
        a.extend_from_slice(args);
        put(self.o, op, &a);
        r
    }
    fn u(&self, v: u32) -> u32 {
        self.ids.u[&v]
    }
    fn fc(&self, v: f32) -> u32 {
        self.ids.f[&v.to_bits()]
    }

    /// floor(x * 2^(24 - e) + 0.5) for x >= 0 (exact: x * 2^(25 - e) truncated, then halved rounding up)
    fn round(&mut self, x: u32, e: u32) -> u32 {
        let (uint, float) = (self.ids.uint, self.ids.float);
        let k = self.op(OP_ISUB, uint, &[self.u(152), e]);
        let kb = self.op(OP_SHIFT_LEFT_LOGICAL, uint, &[k, self.u(23)]);
        let kf = self.op(OP_BITCAST, float, &[kb]);
        let p = self.op(OP_FMUL, float, &[x, kf]);
        let pu = self.op(OP_CONVERT_F_TO_U, uint, &[p]);
        let q = self.op(OP_IADD, uint, &[pu, self.u(1)]);
        self.op(OP_SHIFT_RIGHT_LOGICAL, uint, &[q, self.u(1)])
    }

    /// The RGB9E5 word of float components `c` (r, g, b).
    fn pack(&mut self, c: [u32; 3]) -> u32 {
        let (uint, float, b) = (self.ids.uint, self.ids.float, self.ids.boolean);
        let (z, mx) = (self.fc(0.0), self.fc(SHAREDEXP_MAX));
        let c = c.map(|x| {
            // NaN and negatives to 0 (an ordered compare is false for NaN), then +Inf and large to the maximum
            let gt = self.op(OP_FORD_GREATER_THAN, b, &[x, z]);
            let a = self.op(OP_SELECT, float, &[gt, x, z]);
            let lt = self.op(OP_FORD_LESS_THAN, b, &[a, mx]);
            self.op(OP_SELECT, float, &[lt, a, mx])
        });
        let g1 = self.op(OP_FORD_GREATER_THAN, b, &[c[0], c[1]]);
        let m1 = self.op(OP_SELECT, float, &[g1, c[0], c[1]]);
        let g2 = self.op(OP_FORD_GREATER_THAN, b, &[m1, c[2]]);
        let m = self.op(OP_SELECT, float, &[g2, m1, c[2]]);
        let mb = self.op(OP_BITCAST, uint, &[m]);
        let biased = self.op(OP_SHIFT_RIGHT_LOGICAL, uint, &[mb, self.u(23)]);
        let big = self.op(OP_UGREATER_THAN, b, &[biased, self.u(111)]);
        let sub = self.op(OP_ISUB, uint, &[biased, self.u(111)]);
        let e0 = self.op(OP_SELECT, uint, &[big, sub, self.u(0)]);
        let ms = self.round(m, e0);
        let over = self.op(OP_IEQUAL, b, &[ms, self.u(512)]);
        let e1 = self.op(OP_IADD, uint, &[e0, self.u(1)]);
        let e = self.op(OP_SELECT, uint, &[over, e1, e0]);
        let s = c.map(|x| self.round(x, e));
        let g = self.op(OP_SHIFT_LEFT_LOGICAL, uint, &[s[1], self.u(9)]);
        let bl = self.op(OP_SHIFT_LEFT_LOGICAL, uint, &[s[2], self.u(18)]);
        let el = self.op(OP_SHIFT_LEFT_LOGICAL, uint, &[e, self.u(27)]);
        let rg = self.op(OP_BITWISE_OR, uint, &[s[0], g]);
        let rgb = self.op(OP_BITWISE_OR, uint, &[rg, bl]);
        self.op(OP_BITWISE_OR, uint, &[rgb, el])
    }

    /// The float components (r, g, b) of RGB9E5 word `v`.
    fn unpack(&mut self, v: u32) -> [u32; 3] {
        let (uint, float) = (self.ids.uint, self.ids.float);
        let e = self.op(OP_SHIFT_RIGHT_LOGICAL, uint, &[v, self.u(27)]);
        let k = self.op(OP_IADD, uint, &[e, self.u(103)]);
        let kb = self.op(OP_SHIFT_LEFT_LOGICAL, uint, &[k, self.u(23)]);
        let kf = self.op(OP_BITCAST, float, &[kb]);
        let s9 = self.op(OP_SHIFT_RIGHT_LOGICAL, uint, &[v, self.u(9)]);
        let s18 = self.op(OP_SHIFT_RIGHT_LOGICAL, uint, &[v, self.u(18)]);
        [v, s9, s18].map(|x| {
            let m = self.op(OP_BITWISE_AND, uint, &[x, self.u(511)]);
            let f = self.op(OP_CONVERT_U_TO_F, float, &[m]);
            self.op(OP_FMUL, float, &[f, kf])
        })
    }
}

/// Word positions of an instruction's operands (after the opcode word) that are literals, not ids; the rest are
/// checked for uses of the rewritten variables and loads.
fn literal_operand(op: u32, k: usize) -> bool {
    match op {
        OP_NAME | OP_LINE | OP_LOOP_MERGE | OP_SELECTION_MERGE => true,
        OP_EXT_INST => k == 3,
        OP_COMPOSITE_EXTRACT => k >= 3,
        OP_COMPOSITE_INSERT => k >= 4,
        OP_VECTOR_SHUFFLE => k >= 4,
        OP_SWITCH => k >= 2,
        OP_LOAD => k >= 3,
        62 /* OpStore */ => k >= 2,
        OP_IMAGE_WRITE => k == 3,
        OP_IMAGE_READ => k == 4,
        _ => false,
    }
}

/// `words` with the storage images at (`set`, binding) of `binds` read and written through their emulated view
/// format (see the module comment); None when one is not a plain formatless float storage image variable used only by
/// loads, reads, writes and size queries (then no variant is made).
pub fn rewrite(words: &[u32], set: u32, binds: &[(u32, Emul)]) -> Option<Vec<u32>> {
    if words.len() < 5 || words[0] != 0x0723_0203 || binds.is_empty() {
        return None;
    }
    let mut v: Vec<(usize, u32, usize)> = Vec::new(); // (start, opcode, word count)
    let mut i = 5;
    while i < words.len() {
        let wc = (words[i] >> 16) as usize;
        if wc == 0 || i + wc > words.len() {
            return None;
        }
        v.push((i, words[i] & 0xffff, wc));
        i += wc;
    }
    let a = |x: &(usize, u32, usize)| &words[x.0 + 1..x.0 + x.2];
    let mut bound = words[3];
    let (mut dset, mut dbind): (HashMap<u32, u32>, HashMap<u32, u32>) = Default::default();
    let (mut uint, mut float, mut boolean, mut v4u) = (0, 0, 0, 0);
    let mut images: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut ptrs: HashMap<u32, (u32, u32)> = HashMap::new();
    let mut vars: HashMap<u32, u32> = HashMap::new(); // variable -> pointer type
    let mut first_fn = 0;
    for x in &v {
        let o = a(x);
        match x.1 {
            OP_DECORATE if o.len() == 3 && o[1] == DEC_SET => {
                dset.insert(o[0], o[2]);
            }
            OP_DECORATE if o.len() == 3 && o[1] == DEC_BINDING => {
                dbind.insert(o[0], o[2]);
            }
            OP_TYPE_INT if o[1] == 32 && o[2] == 0 => uint = o[0],
            OP_TYPE_FLOAT if o[1] == 32 => float = o[0],
            OP_TYPE_BOOL => boolean = o[0],
            OP_TYPE_VECTOR if uint != 0 && o[1] == uint && o[2] == 4 => v4u = o[0],
            OP_TYPE_IMAGE => {
                images.insert(o[0], o[1..].to_vec());
            }
            OP_TYPE_POINTER => {
                ptrs.insert(o[0], (o[1], o[2]));
            }
            OP_VARIABLE if first_fn == 0 && o[2] == SC_UNIFORM_CONSTANT => {
                vars.insert(o[1], o[0]);
            }
            OP_FUNCTION if first_fn == 0 => first_fn = x.0,
            _ => {}
        }
    }
    if float == 0 || first_fn == 0 {
        return None;
    }
    // the variables, and the r32ui image type standing in for each of their image types
    let mut targets: HashMap<u32, Emul> = HashMap::new();
    let mut new_img: HashMap<u32, u32> = HashMap::new(); // old image type -> new image type
    let mut decl = Vec::new(); // new types and constants
    let fresh = |bound: &mut u32| {
        *bound += 1;
        *bound - 1
    };
    if uint == 0 {
        uint = fresh(&mut bound);
        put(&mut decl, OP_TYPE_INT, &[uint, 32, 0]);
    }
    if boolean == 0 {
        boolean = fresh(&mut bound);
        put(&mut decl, OP_TYPE_BOOL, &[boolean]);
    }
    if v4u == 0 {
        v4u = fresh(&mut bound);
        put(&mut decl, OP_TYPE_VECTOR, &[v4u, uint, 4]);
    }
    for &(b, e) in binds {
        let var = *vars
            .keys()
            .find(|id| dset.get(id) == Some(&set) && dbind.get(id) == Some(&b))?;
        let &(sc, img) = ptrs.get(&vars[&var])?;
        let t = images.get(&img)?;
        // [sampled type, dim, depth, arrayed, ms, sampled, format]
        if sc != SC_UNIFORM_CONSTANT
            || t.len() != 7
            || t[0] != float
            || t[5] != 2
            || t[1] == DIM_BUFFER
            || t[1] == DIM_SUBPASS
        {
            return None;
        }
        targets.insert(var, e);
        if let std::collections::hash_map::Entry::Vacant(slot) = new_img.entry(img) {
            let want = [uint, t[1], t[2], t[3], t[4], 2, FMT_R32UI];
            let id = match images.iter().find(|(_, o)| o[..] == want[..]) {
                Some((&id, _)) => id,
                None => {
                    let id = fresh(&mut bound);
                    let mut ops = vec![id];
                    ops.extend_from_slice(&want);
                    put(&mut decl, OP_TYPE_IMAGE, &ops);
                    images.insert(id, want.to_vec());
                    id
                }
            };
            slot.insert(id);
        }
    }
    // a pointer type to each new image type
    let mut new_ptr: HashMap<u32, u32> = HashMap::new(); // new image type -> its pointer type
    for &ni in new_img.values() {
        if new_ptr.contains_key(&ni) {
            continue;
        }
        let id = match ptrs.iter().find(|(_, &p)| p == (SC_UNIFORM_CONSTANT, ni)) {
            Some((&id, _)) => id,
            None => {
                let id = fresh(&mut bound);
                put(&mut decl, OP_TYPE_POINTER, &[id, SC_UNIFORM_CONSTANT, ni]);
                id
            }
        };
        new_ptr.insert(ni, id);
    }
    let mut u = HashMap::new();
    for c in UINT_CONSTS {
        let id = fresh(&mut bound);
        put(&mut decl, OP_CONSTANT, &[uint, id, c]);
        u.insert(c, id);
    }
    let mut f = HashMap::new();
    for c in FLOAT_CONSTS {
        let id = fresh(&mut bound);
        put(&mut decl, OP_CONSTANT, &[float, id, c.to_bits()]);
        f.insert(c.to_bits(), id);
    }
    // the loads of the variables (in function bodies), and every other use refused
    let mut loads: HashMap<u32, Emul> = HashMap::new();
    let mut moved = Vec::new();
    for x in &v {
        let o = a(x);
        if x.0 < first_fn {
            if x.1 == OP_VARIABLE && targets.contains_key(&o[1]) {
                let img = ptrs[&o[0]].1;
                let mut ops = o.to_vec();
                ops[0] = new_ptr[&new_img[&img]];
                put(&mut moved, OP_VARIABLE, &ops);
            }
            continue;
        }
        if x.1 == OP_LOAD {
            if let Some(&e) = targets.get(&o[2]) {
                loads.insert(o[1], e);
                continue;
            }
        }
        let allowed = |k: usize| match x.1 {
            OP_IMAGE_WRITE => k == 0,
            OP_IMAGE_READ => k == 2,
            OP_IMAGE_QUERY_SIZE_LOD
            | OP_IMAGE_QUERY_SIZE
            | OP_IMAGE_QUERY_LEVELS
            | OP_IMAGE_QUERY_SAMPLES => k == 2,
            _ => false,
        };
        for (k, w) in o.iter().enumerate() {
            if literal_operand(x.1, k) {
                continue;
            }
            if targets.contains_key(w) || (loads.contains_key(w) && !allowed(k)) {
                return None;
            }
        }
    }
    let ids = Ids {
        uint,
        float,
        boolean,
        u,
        f,
    };
    let mut types: HashMap<u32, (u32, u32)> = HashMap::new(); // vector type -> (component type, count)
    for x in &v {
        let o = a(x);
        if x.1 == OP_TYPE_VECTOR {
            types.insert(o[0], (o[1], o[2]));
        }
    }
    let comps = |ty: u32| -> Option<u32> {
        if ty == float {
            Some(1)
        } else {
            types.get(&ty).filter(|t| t.0 == float).map(|t| t.1)
        }
    };
    // the texel type of every id written (the result types of the function bodies' instructions, and of global
    // constants and undefs: a constant texel stored)
    let mut result_ty: HashMap<u32, u32> = HashMap::new();
    for x in &v {
        let o = a(x);
        let global_value = matches!(x.1, 1 | 43 | 44 | 46); // OpUndef, OpConstant, OpConstantComposite, OpConstantNull
        if (x.0 >= first_fn || global_value) && o.len() >= 2 && has_result_type(x.1) {
            result_ty.insert(o[1], o[0]);
        }
    }
    let mut out = words[..5].to_vec();
    for x in &v {
        let o = a(x);
        if x.0 == first_fn {
            out.extend_from_slice(&decl);
            out.extend_from_slice(&moved);
        }
        if x.0 < first_fn {
            if x.1 == OP_VARIABLE && targets.contains_key(&o[1]) {
                continue;
            }
            out.extend_from_slice(&words[x.0..x.0 + x.2]);
            continue;
        }
        match x.1 {
            OP_LOAD if loads.contains_key(&o[1]) => {
                let img = ptrs[&vars[&o[2]]].1;
                let mut ops = o.to_vec();
                ops[0] = new_img[&img];
                put(&mut out, OP_LOAD, &ops);
            }
            OP_IMAGE_WRITE if loads.contains_key(&o[0]) => {
                let n = comps(*result_ty.get(&o[2])?)?;
                let mut em = Emit {
                    o: &mut out,
                    bound: &mut bound,
                    ids: &ids,
                };
                let c: Vec<u32> = (0..3)
                    .map(|k| {
                        if n == 1 {
                            if k == 0 {
                                o[2]
                            } else {
                                em.fc(0.0)
                            }
                        } else if k < n {
                            em.op(OP_COMPOSITE_EXTRACT, float, &[o[2], k])
                        } else {
                            em.fc(0.0)
                        }
                    })
                    .collect();
                let p = match loads[&o[0]] {
                    Emul::Rgb9e5 => em.pack([c[0], c[1], c[2]]),
                    Emul::Bits32 => em.op(OP_BITCAST, uint, &[c[0]]),
                };
                let t = em.op(OP_COMPOSITE_CONSTRUCT, v4u, &[p, p, p, p]);
                let mut ops = o.to_vec();
                ops[2] = t;
                put(&mut out, OP_IMAGE_WRITE, &ops);
            }
            OP_IMAGE_READ if loads.contains_key(&o[2]) => {
                let n = comps(o[0])?;
                let mut ops = o.to_vec();
                ops[0] = v4u;
                ops[1] = bound;
                bound += 1;
                put(&mut out, OP_IMAGE_READ, &ops);
                let mut em = Emit {
                    o: &mut out,
                    bound: &mut bound,
                    ids: &ids,
                };
                let w = em.op(OP_COMPOSITE_EXTRACT, uint, &[ops[1], 0]);
                let c = match loads[&o[2]] {
                    Emul::Rgb9e5 => em.unpack(w),
                    Emul::Bits32 => [em.op(OP_BITCAST, float, &[w]), em.fc(0.0), em.fc(0.0)],
                };
                let one = em.fc(1.0);
                let all = [c[0], c[1], c[2], one];
                if n == 1 {
                    // the scalar read: its result is the red component, by the original id
                    put(&mut out, 83 /* OpCopyObject */, &[float, o[1], all[0]]);
                } else {
                    let mut ops = vec![o[0], o[1]];
                    ops.extend_from_slice(&all[..n as usize]);
                    put(&mut out, OP_COMPOSITE_CONSTRUCT, &ops);
                }
            }
            _ => out.extend_from_slice(&words[x.0..x.0 + x.2]),
        }
    }
    out[3] = bound;
    Some(out)
}

/// Whether a function-body instruction of opcode `op` with two or more operands starts with a result type and a
/// result id: every one but these (a wrong answer only makes the texel's type unknown, and then no variant is made).
fn has_result_type(op: u32) -> bool {
    !matches!(
        op,
        OP_LINE
            | 62 // OpStore
            | 63 // OpCopyMemory
            | 64 // OpCopyMemorySized
            | OP_IMAGE_WRITE
            | 224 // OpControlBarrier
            | 225 // OpMemoryBarrier
            | 228 // OpAtomicStore
            | OP_LOOP_MERGE
            | OP_SELECTION_MERGE
            | 250 // OpBranchConditional
            | OP_SWITCH
            | 5294 // OpEmitMeshTasksEXT
            | 5295 // OpSetMeshOutputsEXT
    )
}

/// A stage's storage images bound to emulated views, as the variant's tag: (binding, conversion) pairs, one per
/// binding, sorted.
pub fn tag(binds: &[(u32, Emul)]) -> Vec<u32> {
    let mut seen = HashSet::new();
    let mut b: Vec<(u32, Emul)> = binds.iter().copied().filter(|b| seen.insert(b.0)).collect();
    b.sort_unstable_by_key(|b| b.0);
    b.into_iter().flat_map(|(b, e)| [b, e as u32]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The spec's formula in f64 (log2 and powers taken literally), NaN as 0.
    fn spec_pack(rgb: [f32; 3]) -> u32 {
        let c = rgb.map(|x| {
            let x = x as f64;
            if x.is_nan() {
                0.0
            } else {
                x.clamp(0.0, 65408.0)
            }
        });
        let m = c[0].max(c[1]).max(c[2]);
        let ep = m.log2().floor().max(-16.0) + 16.0;
        let ms = (m / 2f64.powf(ep - 24.0) + 0.5).floor();
        let e = if ms == 512.0 { ep + 1.0 } else { ep };
        let s = c.map(|x| (x / 2f64.powf(e - 24.0) + 0.5).floor() as u32);
        assert!(s.iter().all(|&s| s < 512) && (0.0..32.0).contains(&e));
        s[0] | s[1] << 9 | s[2] << 18 | (e as u32) << 27
    }

    /// xorshift: inputs over the whole float range and near the encoding's boundaries
    fn inputs() -> Vec<[f32; 3]> {
        let mut s = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s
        };
        let mut v = vec![];
        for _ in 0..20000 {
            let r = next();
            let k = (r >> 60) as u32;
            let f = |b: u64| -> f32 {
                match k % 4 {
                    // any bit pattern (NaN, Inf, negatives, denormals)
                    0 => f32::from_bits(b as u32),
                    // [2^-30, 2^17)
                    1 => f32::from_bits(((97 + (b >> 24) % 47) << 23 | b & 0x7f_ffff) as u32),
                    // a half step of the 9-bit mantissa at some exponent (round half up)
                    2 => ((b & 511) as f32 + 0.5) * 2f32.powi((b >> 9 & 31) as i32 - 24),
                    _ => (b & 0xffff) as f32 / 256.0,
                }
            };
            v.push([f(r), f(next()), f(next())]);
        }
        v
    }

    #[test]
    fn rgb9e5_reference_values() {
        assert_eq!(pack_rgb9e5([0.0; 3]), 0);
        assert_eq!(pack_rgb9e5([1.0; 3]), 256 | 256 << 9 | 256 << 18 | 16 << 27);
        assert_eq!(pack_rgb9e5([65408.0; 3]), u32::MAX, "sharedexp_max");
        assert_eq!(
            pack_rgb9e5([1e9, f32::INFINITY, 70000.0]),
            u32::MAX,
            "clamped to the maximum"
        );
        assert_eq!(
            pack_rgb9e5([-1.0, -0.0, f32::NEG_INFINITY]),
            0,
            "negatives clamp to 0"
        );
        assert_eq!(
            pack_rgb9e5([f32::NAN, 1.0, 0.0]),
            256 << 9 | 16 << 27,
            "NaN as 0"
        );
        // the smallest step 2^-24, and half of it rounding up
        assert_eq!(pack_rgb9e5([2f32.powi(-24), 0.0, 0.0]), 1);
        assert_eq!(pack_rgb9e5([2f32.powi(-25), 0.0, 0.0]), 1);
        assert_eq!(pack_rgb9e5([2f32.powi(-26), 0.0, 0.0]), 0);
        assert_eq!(
            pack_rgb9e5([f32::from_bits(1), 0.0, 0.0]),
            0,
            "a float denormal"
        );
        // max_s rounding up to 2^N moves the exponent: 511.5 * 2^-8 -> 256 * 2^-7
        assert_eq!(pack_rgb9e5([511.5 / 256.0, 0.0, 0.0]), 256 | 17 << 27);
        assert_eq!(unpack_rgb9e5(256 | 17 << 27), [2.0, 0.0, 0.0]);
        assert_eq!(unpack_rgb9e5(u32::MAX), [65408.0; 3]);
        assert_eq!(unpack_rgb9e5(1), [2f32.powi(-24), 0.0, 0.0]);
        for x in inputs() {
            assert_eq!(pack_rgb9e5(x), spec_pack(x), "{x:?}");
        }
        // representable values come back exactly
        for x in inputs() {
            let p = pack_rgb9e5(x);
            assert_eq!(pack_rgb9e5(unpack_rgb9e5(p)), p, "{x:?}");
        }
    }

    // ---- a compute shader writing (and reading) a formatless float storage image, and an evaluator for the result

    const IMG_SET: u32 = 5;
    const VOID: u32 = 1;
    const FNTY: u32 = 2;
    const F32: u32 = 3;
    const I32: u32 = 4;
    const V2I: u32 = 5;
    const V4F: u32 = 6;
    const IMG_T: u32 = 7;
    const PTR: u32 = 8;
    const VAR_A: u32 = 9; // binding 128 (emulated)
    const VAR_B: u32 = 10; // binding 129 (not)
    const MAIN: u32 = 11;
    const C: [u32; 4] = [12, 13, 14, 15];
    const I0: u32 = 16;
    const COORD: u32 = 17;
    const LABEL: u32 = 18;

    /// `read`: reads binding 128 and writes what it read to binding 129; else writes `texel` to binding 128.
    fn fixture(texel: [f32; 4], read: bool, extra_use: bool) -> Vec<u32> {
        let mut w = vec![0x0723_0203, 0x0001_0300, 0, 0, 0];
        put(&mut w, 17, &[1]); // Shader
        put(&mut w, 17, &[55]); // StorageImageReadWithoutFormat
        put(&mut w, 17, &[56]); // StorageImageWriteWithoutFormat
        put(&mut w, 17, &[50]); // ImageQuery
        put(&mut w, 14, &[0, 1]);
        put(&mut w, 15, &[5, MAIN, u32::from_le_bytes(*b"main"), 0]);
        put(&mut w, 16, &[MAIN, 17, 1, 1, 1]);
        for (v, b) in [(VAR_A, 128), (VAR_B, 129)] {
            put(&mut w, OP_DECORATE, &[v, DEC_SET, IMG_SET]);
            put(&mut w, OP_DECORATE, &[v, DEC_BINDING, b]);
        }
        put(&mut w, 19, &[VOID]);
        put(&mut w, 33, &[FNTY, VOID]);
        put(&mut w, OP_TYPE_FLOAT, &[F32, 32]);
        put(&mut w, OP_TYPE_INT, &[I32, 32, 1]);
        put(&mut w, OP_TYPE_VECTOR, &[V2I, I32, 2]);
        put(&mut w, OP_TYPE_VECTOR, &[V4F, F32, 4]);
        put(&mut w, OP_TYPE_IMAGE, &[IMG_T, F32, 1, 0, 0, 0, 2, 0]);
        put(&mut w, OP_TYPE_POINTER, &[PTR, SC_UNIFORM_CONSTANT, IMG_T]);
        put(&mut w, OP_VARIABLE, &[PTR, VAR_A, SC_UNIFORM_CONSTANT]);
        put(&mut w, OP_VARIABLE, &[PTR, VAR_B, SC_UNIFORM_CONSTANT]);
        for k in 0..4 {
            put(&mut w, OP_CONSTANT, &[F32, C[k], texel[k].to_bits()]);
        }
        put(&mut w, OP_CONSTANT, &[I32, I0, 0]);
        put(&mut w, 44, &[V2I, COORD, I0, I0]);
        put(&mut w, OP_FUNCTION, &[VOID, MAIN, 0, FNTY]);
        put(&mut w, 248, &[LABEL]);
        let mut id = 19;
        let mut next = || {
            id += 1;
            id - 1
        };
        let a = next();
        put(&mut w, OP_LOAD, &[IMG_T, a, VAR_A]);
        let q = next();
        put(&mut w, OP_IMAGE_QUERY_SIZE, &[V2I, q, a]);
        if read {
            let r = next();
            put(&mut w, OP_IMAGE_READ, &[V4F, r, a, COORD]);
            let b = next();
            put(&mut w, OP_LOAD, &[IMG_T, b, VAR_B]);
            put(&mut w, OP_IMAGE_WRITE, &[b, COORD, r]);
        } else {
            let t = next();
            put(
                &mut w,
                OP_COMPOSITE_CONSTRUCT,
                &[V4F, t, C[0], C[1], C[2], C[3]],
            );
            put(&mut w, OP_IMAGE_WRITE, &[a, COORD, t]);
        }
        if extra_use {
            put(&mut w, 83, &[IMG_T, next(), a]); // OpCopyObject of the image: not rewritten
        }
        put(&mut w, 253, &[]);
        put(&mut w, 56, &[]);
        w[3] = id;
        w
    }

    /// Runs the entry function's straight-line code: the texel words each image variable is written with. Every
    /// image read returns `content` in all components.
    fn eval(w: &[u32], content: u32) -> HashMap<u32, Vec<u32>> {
        let mut val: HashMap<u32, Vec<u32>> = HashMap::new();
        let mut loaded: HashMap<u32, u32> = HashMap::new();
        let mut written = HashMap::new();
        let mut i = 5;
        while i < w.len() {
            let n = (w[i] >> 16) as usize;
            let (op, o) = (w[i] & 0xffff, &w[i + 1..i + n]);
            i += n;
            let f = |v: &HashMap<u32, Vec<u32>>, id: u32| f32::from_bits(v[&id][0]);
            let u = |v: &HashMap<u32, Vec<u32>>, id: u32| v[&id][0];
            let r: Vec<u32> = match op {
                OP_CONSTANT => vec![o[2]],
                44 | OP_COMPOSITE_CONSTRUCT => o[2..].iter().flat_map(|x| val[x].clone()).collect(),
                OP_LOAD => {
                    loaded.insert(o[1], o[2]);
                    continue;
                }
                OP_IMAGE_WRITE => {
                    written.insert(loaded[&o[0]], val[&o[2]].clone());
                    continue;
                }
                OP_IMAGE_READ => vec![content; 4],
                OP_COMPOSITE_EXTRACT => vec![val[&o[2]][o[3] as usize]],
                83 | OP_BITCAST => val[&o[2]].clone(),
                OP_CONVERT_F_TO_U => vec![f(&val, o[2]) as u32],
                OP_CONVERT_U_TO_F => vec![(u(&val, o[2]) as f32).to_bits()],
                OP_FMUL => vec![(f(&val, o[2]) * f(&val, o[3])).to_bits()],
                OP_IADD => vec![u(&val, o[2]).wrapping_add(u(&val, o[3]))],
                OP_ISUB => vec![u(&val, o[2]).wrapping_sub(u(&val, o[3]))],
                OP_SHIFT_LEFT_LOGICAL => vec![u(&val, o[2]) << u(&val, o[3])],
                OP_SHIFT_RIGHT_LOGICAL => vec![u(&val, o[2]) >> u(&val, o[3])],
                OP_BITWISE_OR => vec![u(&val, o[2]) | u(&val, o[3])],
                OP_BITWISE_AND => vec![u(&val, o[2]) & u(&val, o[3])],
                OP_IEQUAL => vec![(u(&val, o[2]) == u(&val, o[3])) as u32],
                OP_UGREATER_THAN => vec![(u(&val, o[2]) > u(&val, o[3])) as u32],
                OP_FORD_GREATER_THAN => vec![(f(&val, o[2]) > f(&val, o[3])) as u32],
                OP_FORD_LESS_THAN => vec![(f(&val, o[2]) < f(&val, o[3])) as u32],
                OP_SELECT => {
                    if u(&val, o[2]) != 0 {
                        val[&o[3]].clone()
                    } else {
                        val[&o[4]].clone()
                    }
                }
                _ => continue,
            };
            val.insert(o[1], r);
        }
        written
    }

    fn spirv_val(w: &[u32]) {
        let exe = std::env::var_os("NXGFX_SPIRV_VAL")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                let p = std::path::Path::new(&std::env::var_os("VULKAN_SDK")?)
                    .join("Bin/spirv-val.exe");
                p.is_file().then_some(p)
            });
        let Some(exe) = exe else {
            eprintln!(
                "spirv-val not found (VULKAN_SDK / NXGFX_SPIRV_VAL): the module is not validated"
            );
            return;
        };
        let p =
            std::env::temp_dir().join(format!("storemu-{}-{}.spv", std::process::id(), w.len()));
        std::fs::write(
            &p,
            w.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>(),
        )
        .unwrap();
        let out = std::process::Command::new(exe)
            .args(["--target-env", "vulkan1.1"])
            .arg(&p)
            .output()
            .unwrap();
        let _ = std::fs::remove_file(&p);
        assert!(
            out.status.success(),
            "spirv-val: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn writes_pack_and_validate() {
        let base = fixture([1.0, 2.0, 3.0, 4.0], false, false);
        spirv_val(&base);
        let out = rewrite(&base, IMG_SET, &[(128, Emul::Rgb9e5)]).unwrap();
        spirv_val(&out);
        for x in inputs().into_iter().step_by(7) {
            let w = rewrite(
                &fixture([x[0], x[1], x[2], 9.0], false, false),
                IMG_SET,
                &[(128, Emul::Rgb9e5)],
            )
            .unwrap();
            let got = &eval(&w, 0)[&VAR_A];
            assert_eq!(got, &vec![pack_rgb9e5(x); 4], "{x:?}");
        }
    }

    #[test]
    fn bits_mode_writes_and_reads_the_word() {
        // Ocarina's lighting pass: the word packed by the game, stored as the bits of a float in R
        let word = 0x8c12_3456u32;
        let w = fixture([f32::from_bits(word), 0.0, 0.0, 0.0], false, false);
        let out = rewrite(&w, IMG_SET, &[(128, Emul::Bits32)]).unwrap();
        spirv_val(&out);
        assert_eq!(eval(&out, 0)[&VAR_A], vec![word; 4]);
        let r = rewrite(
            &fixture([0.0; 4], true, false),
            IMG_SET,
            &[(128, Emul::Bits32)],
        )
        .unwrap();
        spirv_val(&r);
        let got = &eval(&r, word)[&VAR_B];
        assert_eq!(got, &vec![word, 0, 0, 1f32.to_bits()]);
        assert_eq!(
            emulation(
                vk::Format::E5B9G9R9_UFLOAT_PACK32,
                vk::Format::R32_UINT,
                vk::Format::R32_UINT
            ),
            Some(Emul::Bits32)
        );
        assert_eq!(
            emulation(
                vk::Format::E5B9G9R9_UFLOAT_PACK32,
                vk::Format::E5B9G9R9_UFLOAT_PACK32,
                vk::Format::R32_UINT
            ),
            Some(Emul::Rgb9e5)
        );
        assert_eq!(
            emulation(
                vk::Format::R32_UINT,
                vk::Format::R32_UINT,
                vk::Format::R32_UINT
            ),
            None,
            "not emulated"
        );
        assert_eq!(
            emulation(
                vk::Format::E5B9G9R9_UFLOAT_PACK32,
                vk::Format::R32_SFLOAT,
                vk::Format::R32_SFLOAT
            ),
            None
        );
        // a non-table image viewed as E5B9G9R9 gets the twin view (`image_view_kind` remaps by the requested
        // format): the requested format decides, it packs
        assert_eq!(
            emulation(
                vk::Format::R32_UINT,
                vk::Format::E5B9G9R9_UFLOAT_PACK32,
                vk::Format::R32_UINT
            ),
            Some(Emul::Rgb9e5)
        );
    }

    #[test]
    fn reads_unpack_and_validate() {
        let out = rewrite(
            &fixture([0.0; 4], true, false),
            IMG_SET,
            &[(128, Emul::Rgb9e5)],
        )
        .unwrap();
        spirv_val(&out);
        for v in [0, 1, u32::MAX, 256 | 17 << 27, 0x1234_5678, 0xdead_beef] {
            let got = &eval(&out, v)[&VAR_B];
            let want = unpack_rgb9e5(v);
            let want: Vec<u32> = [want[0], want[1], want[2], 1.0]
                .iter()
                .map(|x| x.to_bits())
                .collect();
            assert_eq!(got, &want, "{v:#x}");
        }
    }

    /// Every formatless float storage image of a game's translated shaders (`NXGFX_STOREMU_IN`: its `shaders/spv`)
    /// emulated, the variants validated; opt-in: `cargo test -p graphics --lib storemu_folder -- --ignored`.
    #[test]
    #[ignore]
    fn storemu_folder() {
        let Ok(dir) = std::env::var("NXGFX_STOREMU_IN") else {
            return;
        };
        let (mut made, mut refused) = (0, vec![]);
        for e in std::fs::read_dir(&dir).unwrap().flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            let j: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
            let binds: Vec<(u32, u32)> = j["bindings"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|b| {
                    b["type"] == "storage_image"
                        && b["resource"]["format"] == "Unknown"
                        && b["resource"]["type"] != 2
                        && b["count"] == 1
                })
                .map(|b| {
                    (
                        b["set"].as_u64().unwrap() as u32,
                        b["binding"].as_u64().unwrap() as u32,
                    )
                })
                .collect();
            let Some(&(set, _)) = binds.first() else {
                continue;
            };
            let b = std::fs::read(p.with_extension("spv")).unwrap();
            let w: Vec<u32> = b
                .chunks(4)
                .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
                .collect();
            let list: Vec<(u32, Emul)> = binds
                .iter()
                .filter(|b| b.0 == set)
                .map(|b| (b.1, Emul::Rgb9e5))
                .collect();
            match rewrite(&w, set, &list) {
                Some(out) => {
                    spirv_val(&out);
                    made += 1;
                }
                None => refused.push(p.file_stem().unwrap().to_string_lossy().to_string()),
            }
        }
        eprintln!("{made} variants validated; refused: {refused:?}");
    }

    #[test]
    fn only_the_named_plain_bindings_are_rewritten() {
        let w = fixture([0.0; 4], false, false);
        assert!(
            rewrite(&w, IMG_SET, &[(130, Emul::Rgb9e5)]).is_none(),
            "no such binding"
        );
        assert!(
            rewrite(&w, IMG_SET + 1, &[(128, Emul::Rgb9e5)]).is_none(),
            "another set"
        );
        assert!(rewrite(&w, IMG_SET, &[]).is_none());
        let other = fixture([0.0; 4], false, true);
        assert!(
            rewrite(&other, IMG_SET, &[(128, Emul::Rgb9e5)]).is_none(),
            "an image used otherwise"
        );
        // binding 129 keeps its float image type: the read variant writes it with the unpacked floats
        let r = rewrite(
            &fixture([0.0; 4], true, false),
            IMG_SET,
            &[(128, Emul::Rgb9e5)],
        )
        .unwrap();
        assert_eq!(eval(&r, 0)[&VAR_B].len(), 4);
        assert_eq!(
            tag(&[
                (129, Emul::Rgb9e5),
                (128, Emul::Rgb9e5),
                (129, Emul::Rgb9e5)
            ]),
            vec![128, 1, 129, 1]
        );
    }
}
