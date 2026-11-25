//! Transform feedback: the capture variant of a vertex (or geometry) shader.
//!
//! An NVN program's control section says what its last vertex-processing stage captures; the shader build records
//! it in the reflection json as `transform_feedback` (per buffer: its stride and its outputs as [location, components],
//! interleaved in that order). Vulkan captures whole output variables and the translator's outputs are float vec4s,
//! so each captured output with fewer than 4 components gets a variable of its own with that many (a free Location),
//! stored from the original at every return of the entry point; every captured variable gets XfbBuffer, XfbStride
//! and Offset, the module the TransformFeedback capability and the Xfb execution mode.

use super::shader::Module;
use ash::vk;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// One captured output: (buffer, location, components, byte offset in the buffer's record).
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub nbuf: u32,
    pub stride: [u32; 4],
    pub outs: Vec<(u32, u32, u32, u32)>,
}

impl Layout {
    /// From the json's `transform_feedback` array (None: nothing captured, or a form not met yet).
    pub fn from_json(v: &serde_json::Value) -> Option<Layout> {
        let a = v.as_array()?;
        let mut l = Layout {
            nbuf: 0,
            stride: [0; 4],
            outs: vec![],
        };
        for (b, e) in a.iter().enumerate().take(4) {
            let Some(o) = e.as_object() else { continue };
            let stride = o.get("stride")?.as_u64()? as u32;
            l.stride[b] = stride;
            let mut off = 0;
            for x in o.get("outputs")?.as_array()? {
                let loc = x.get(0)?.as_u64()? as u32;
                let n = x.get(1)?.as_u64()? as u32;
                if loc >= 32 || !(1..=4).contains(&n) || l.outs.len() == 16 {
                    return None;
                }
                l.outs.push((b as u32, loc, n, off));
                off += 4 * n;
            }
            if off > stride {
                return None;
            }
            l.nbuf = b as u32 + 1;
        }
        (!l.outs.is_empty()).then_some(l)
    }
}

const OP_ENTRY_POINT: u32 = 15;
const OP_EXEC_MODE: u32 = 16;
const OP_CAPABILITY: u32 = 17;
const OP_TYPE_FLOAT: u32 = 22;
const OP_TYPE_VECTOR: u32 = 23;
const OP_TYPE_POINTER: u32 = 32;
const OP_FUNCTION: u32 = 54;
const OP_VARIABLE: u32 = 59;
const OP_LOAD: u32 = 61;
const OP_STORE: u32 = 62;
const OP_DECORATE: u32 = 71;
const OP_MEMBER_DECORATE: u32 = 72;
const OP_VECTOR_SHUFFLE: u32 = 79;
const OP_COMPOSITE_EXTRACT: u32 = 81;
const OP_RETURN: u32 = 253;
const DEC_LOCATION: u32 = 30;
const DEC_OFFSET: u32 = 35;
const DEC_XFB_BUFFER: u32 = 36;
const DEC_XFB_STRIDE: u32 = 37;
const CAP_XFB: u32 = 53;
const MODE_XFB: u32 = 11;
const SC_OUTPUT: u32 = 3;

fn put(o: &mut Vec<u32>, op: u32, ops: &[u32]) {
    o.push(((ops.len() as u32 + 1) << 16) | op);
    o.extend_from_slice(ops);
}

/// The capture variant of a module's words (None: its outputs are not what the layout names).
pub fn variant(w: &[u32], l: &Layout) -> Option<Vec<u32>> {
    if w.len() < 5 || w[0] != 0x0723_0203 {
        return None;
    }
    let mut bound = w[3];
    let (mut f32t, mut vec, mut ptr) = (0u32, [0u32; 5], [0u32; 5]);
    let (mut entry, mut entry_at, mut last_cap, mut last_dec, mut first_fn) =
        (0u32, 0usize, 0usize, 0usize, 0usize);
    let mut loc_of: HashMap<u32, u32> = HashMap::new();
    let mut outs: Vec<(u32, u32)> = vec![]; // (variable, pointer type)
    let mut ptr_to: HashMap<u32, u32> = HashMap::new();
    let mut i = 5;
    while i < w.len() {
        let (wc, op) = ((w[i] >> 16) as usize, w[i] & 0xffff);
        if wc == 0 || i + wc > w.len() {
            return None;
        }
        match op {
            OP_CAPABILITY => last_cap = i + wc,
            OP_ENTRY_POINT if entry == 0 => {
                entry = w[i + 2];
                entry_at = i;
            }
            OP_DECORATE | OP_MEMBER_DECORATE => {
                last_dec = i + wc;
                if op == OP_DECORATE && wc == 4 && w[i + 2] == DEC_LOCATION {
                    loc_of.insert(w[i + 1], w[i + 3]);
                }
            }
            OP_TYPE_FLOAT if w[i + 2] == 32 => f32t = w[i + 1],
            OP_TYPE_VECTOR if f32t != 0 && w[i + 2] == f32t && w[i + 3] <= 4 => {
                vec[w[i + 3] as usize] = w[i + 1]
            }
            OP_TYPE_POINTER => {
                ptr_to.insert(w[i + 1], w[i + 3]);
                if w[i + 2] == SC_OUTPUT {
                    if w[i + 3] == f32t {
                        ptr[1] = w[i + 1];
                    }
                    for c in 2..=4 {
                        if vec[c] != 0 && w[i + 3] == vec[c] {
                            ptr[c] = w[i + 1];
                        }
                    }
                }
            }
            OP_VARIABLE if wc >= 4 && w[i + 3] == SC_OUTPUT => outs.push((w[i + 2], w[i + 1])),
            OP_FUNCTION if first_fn == 0 => first_fn = i,
            _ => {}
        }
        i += wc;
    }
    if f32t == 0 || vec[4] == 0 || entry == 0 || first_fn == 0 || last_dec == 0 || last_cap == 0 {
        return None;
    }
    let mut used_loc = 0u32;
    let mut src = vec![0u32; l.outs.len()];
    for &(id, ty) in &outs {
        let Some(&loc) = loc_of.get(&id) else {
            continue;
        };
        if loc < 32 {
            used_loc |= 1 << loc;
        }
        for (k, o) in l.outs.iter().enumerate() {
            if o.1 == loc {
                if ptr_to.get(&ty) != Some(&vec[4]) {
                    return None;
                }
                src[k] = id;
            }
        }
    }
    if src.contains(&0) {
        return None; // the shader does not write an output the layout captures
    }
    let (mut decs, mut types) = (vec![], vec![]);
    let mut nv = vec![0u32; l.outs.len()];
    let mut iface = vec![];
    for (k, o) in l.outs.iter().enumerate() {
        let nc = o.2 as usize;
        let mut var = src[k];
        if nc < 4 {
            if nc == 1 && ptr[1] == 0 {
                ptr[1] = bound;
                bound += 1;
                put(&mut types, OP_TYPE_POINTER, &[ptr[1], SC_OUTPUT, f32t]);
            }
            if nc > 1 && vec[nc] == 0 {
                vec[nc] = bound;
                bound += 1;
                put(&mut types, OP_TYPE_VECTOR, &[vec[nc], f32t, nc as u32]);
            }
            if nc > 1 && ptr[nc] == 0 {
                ptr[nc] = bound;
                bound += 1;
                put(&mut types, OP_TYPE_POINTER, &[ptr[nc], SC_OUTPUT, vec[nc]]);
            }
            var = bound;
            bound += 1;
            put(&mut types, OP_VARIABLE, &[ptr[nc], var, SC_OUTPUT]);
            let fl = (0..32).rev().find(|f| used_loc >> f & 1 == 0)?;
            used_loc |= 1 << fl;
            put(&mut decs, OP_DECORATE, &[var, DEC_LOCATION, fl]);
            iface.push(var);
        }
        nv[k] = var;
        put(&mut decs, OP_DECORATE, &[var, DEC_XFB_BUFFER, o.0]);
        put(
            &mut decs,
            OP_DECORATE,
            &[var, DEC_XFB_STRIDE, l.stride[o.0 as usize]],
        );
        put(&mut decs, OP_DECORATE, &[var, DEC_OFFSET, o.3]);
    }
    let mut o = w[..5].to_vec();
    let mut in_entry = false;
    let mut i = 5;
    while i < w.len() {
        let (wc, op) = ((w[i] >> 16) as usize, w[i] & 0xffff);
        if i == last_cap {
            put(&mut o, OP_CAPABILITY, &[CAP_XFB]);
        }
        if i == last_dec {
            o.extend_from_slice(&decs);
        }
        if i == first_fn {
            o.extend_from_slice(&types);
        }
        if op == OP_FUNCTION {
            in_entry = w[i + 2] == entry;
        }
        if i == entry_at {
            o.push(((wc + iface.len()) as u32) << 16 | OP_ENTRY_POINT);
            o.extend_from_slice(&w[i + 1..i + wc]);
            o.extend_from_slice(&iface);
            put(&mut o, OP_EXEC_MODE, &[entry, MODE_XFB]);
            i += wc;
            continue;
        }
        if op == OP_RETURN && in_entry {
            for (k, ol) in l.outs.iter().enumerate() {
                if nv[k] == src[k] {
                    continue;
                }
                let (ld, v) = (bound, bound + 1);
                bound += 2;
                let nc = ol.2;
                put(&mut o, OP_LOAD, &[vec[4], ld, src[k]]);
                if nc == 1 {
                    put(&mut o, OP_COMPOSITE_EXTRACT, &[f32t, v, ld, 0]);
                } else {
                    let mut x = vec![vec[nc as usize], v, ld, ld];
                    x.extend(0..nc);
                    put(&mut o, OP_VECTOR_SHUFFLE, &x);
                }
                put(&mut o, OP_STORE, &[nv[k], v]);
            }
        }
        o.extend_from_slice(&w[i..i + wc]);
        i += wc;
    }
    o[3] = bound;
    Some(o)
}

static VARIANTS: Mutex<Option<HashMap<u64, Option<Arc<Module>>>>> = Mutex::new(None);

/// The capture variant of module `m` (made once; None when it cannot be made, said in red once per shader).
pub fn variant_module(dev: &ash::Device, m: &Arc<Module>) -> Option<Arc<Module>> {
    let l = m.xfb.as_ref()?;
    let mut g = VARIANTS.lock().unwrap();
    let map = g.get_or_insert_with(HashMap::new);
    if let Some(v) = map.get(&m.id) {
        return v.clone();
    }
    let made = variant(&m.words, l).and_then(|w| {
        let ci = vk::ShaderModuleCreateInfo::default().code(&w);
        let module = unsafe { dev.create_shader_module(&ci, None) }.ok()?;
        let mut key = m.key;
        key[7] ^= 0x5a; // the pipeline cache tells the variant from the shader
        Some(Arc::new(Module {
            key,
            stage: m.stage,
            module,
            entry: m.entry.clone(),
            res: m.res.clone(),
            uses_pool: m.uses_pool,
            inputs: m.inputs.clone(),
            c1: m.c1.clone(),
            id: m.id | 1 << 63,
            omap: m.omap,
            mesh: m.mesh,
            xfb: None,
            words: w,
            per_primitive: m.per_primitive.clone(),
            facts: Default::default(),
        }))
    });
    let hex: String = m.key.iter().map(|b| format!("{b:02x}")).collect();
    match &made {
        Some(_) => crate::vklog!("shader {hex}: transform feedback variant ({} outputs into {} buffers, strides {:?})", l.outs.len(), l.nbuf, l.stride),
        None => crate::log::red(&format!("shader {hex}: no transform feedback variant (its outputs are not what the control section names): its captures are not written")),
    }
    map.insert(m.id, made.clone());
    made
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_from_the_reflection() {
        let v: serde_json::Value = serde_json::from_str(
            r#"[{"stride": 12, "outputs": [[0, 3]]}, {"stride": 4, "outputs": [[1, 1]]}, [], []]"#,
        )
        .unwrap();
        let l = Layout::from_json(&v).unwrap();
        assert_eq!(l.nbuf, 2);
        assert_eq!(l.outs, vec![(0, 0, 3, 0), (1, 1, 1, 0)]);
        assert_eq!(l.stride, [12, 4, 0, 0]);
    }
}
