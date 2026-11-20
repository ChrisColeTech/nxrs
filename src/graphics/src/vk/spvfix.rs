//! Shape changes to translated SPIR-V a driver's compiler needs (the program's meaning is unchanged).
//!
//! `late_outputs`: a vertex shader's Location outputs are written to private copies wherever the translated code
//! writes them and stored to the outputs once, before each return of the entry point. The translator writes each
//! output component where the guest program does, often inside branches; NVIDIA's compiler (nvgpucomp64) faulted
//! compiling Donkey Kong Bananza's vertex shader d87ccbede95b9f2c that way (as a pipeline-library part with any pixel
//! shader, and whole with pixel shader 6655320127e098d6), and compiles it with its outputs written once at the end.

use std::collections::HashMap;

const OP_ENTRY_POINT: u32 = 15;
const OP_TYPE_POINTER: u32 = 32;
const OP_CONSTANT_NULL: u32 = 46;
const OP_FUNCTION: u32 = 54;
const OP_VARIABLE: u32 = 59;
const OP_LOAD: u32 = 61;
const OP_STORE: u32 = 62;
const OP_COPY_MEMORY: u32 = 63;
const OP_ACCESS_CHAIN: u32 = 65;
const OP_IN_BOUNDS_ACCESS_CHAIN: u32 = 66;
const OP_DECORATE: u32 = 71;
const OP_RETURN: u32 = 253;
const OP_NAME: u32 = 5;
const OP_LINE: u32 = 8;
const OP_EXT_INST: u32 = 12;
const OP_LOOP_MERGE: u32 = 246;
const OP_SELECTION_MERGE: u32 = 247;
const OP_SWITCH: u32 = 251;
const DEC_LOCATION: u32 = 30;
const SC_OUTPUT: u32 = 3;
const SC_PRIVATE: u32 = 6;

fn ins(op: u32, ops: &[u32]) -> Vec<u32> {
    let mut v = Vec::with_capacity(ops.len() + 1);
    v.push(((ops.len() as u32 + 1) << 16) | op);
    v.extend_from_slice(ops);
    v
}

/// `words` with the Location outputs written once at the end (see above); None when the module has none, or uses an
/// output in a way this pass does not rewrite (then the module is used as it is).
pub fn late_outputs(words: &[u32]) -> Option<Vec<u32>> {
    if words.len() < 5 || words[0] != 0x0723_0203 {
        return None;
    }
    // the instructions: (start, opcode, operands)
    let mut v: Vec<(usize, u32, &[u32])> = Vec::new();
    let mut i = 5;
    while i < words.len() {
        let n = (words[i] >> 16) as usize;
        if n == 0 || i + n > words.len() {
            return None;
        }
        v.push((i, words[i] & 0xffff, &words[i + 1..i + n]));
        i += n;
    }
    let entry = v.iter().find(|x| x.1 == OP_ENTRY_POINT).map(|x| x.2[1])?;
    let mut located = std::collections::HashSet::new();
    for x in &v {
        if x.1 == OP_DECORATE && x.2.len() >= 2 && x.2[1] == DEC_LOCATION {
            located.insert(x.2[0]);
        }
    }
    let mut ptr: HashMap<u32, (u32, u32)> = HashMap::new(); // pointer type -> (storage class, pointee)
    for x in &v {
        if x.1 == OP_TYPE_POINTER {
            ptr.insert(x.2[0], (x.2[1], x.2[2]));
        }
    }
    let mut outs: Vec<(u32, u32)> = Vec::new(); // (variable, its pointee type)
    for x in &v {
        if x.1 == OP_VARIABLE && x.2.len() >= 3 && x.2[2] == SC_OUTPUT && located.contains(&x.2[1]) {
            outs.push((x.2[1], ptr.get(&x.2[0])?.1));
        }
    }
    if outs.is_empty() {
        return None;
    }
    let is_out = |id: u32| outs.iter().any(|o| o.0 == id);
    let mut bound = words[3];
    let mut new_id = || {
        bound += 1;
        bound - 1
    };
    // declarations: a private pointer type per pointee, a null constant per pointee, a shadow per output
    let mut decl: Vec<u32> = Vec::new();
    let mut priv_ptr: HashMap<u32, u32> = HashMap::new();
    let mut nulls: HashMap<u32, u32> = HashMap::new();
    let mut shadow: HashMap<u32, u32> = HashMap::new();
    for &(var, t) in &outs {
        let pp = *priv_ptr.entry(t).or_insert_with(|| {
            let id = new_id();
            decl.extend(ins(OP_TYPE_POINTER, &[id, SC_PRIVATE, t]));
            id
        });
        let nul = *nulls.entry(t).or_insert_with(|| {
            let id = new_id();
            decl.extend(ins(OP_CONSTANT_NULL, &[t, id]));
            id
        });
        let s = new_id();
        decl.extend(ins(OP_VARIABLE, &[pp, s, SC_PRIVATE, nul]));
        shadow.insert(var, s);
    }
    // access chains into an output: a private pointer of the component type (declared with the rest)
    for x in &v {
        if (x.1 == OP_ACCESS_CHAIN || x.1 == OP_IN_BOUNDS_ACCESS_CHAIN) && is_out(x.2[2]) {
            let (sc, c) = *ptr.get(&x.2[0])?;
            if sc != SC_OUTPUT {
                return None;
            }
            if !priv_ptr.contains_key(&(c | 0x8000_0000)) {
                let id = new_id();
                decl.extend(ins(OP_TYPE_POINTER, &[id, SC_PRIVATE, c]));
                priv_ptr.insert(c | 0x8000_0000, id);
            }
        }
    }
    let version = words[1];
    let mut out: Vec<u32> = words[..5].to_vec();
    let mut in_entry = false;
    let mut declared = false;
    for &(at, op, a) in &v {
        let whole = &words[at..at + a.len() + 1];
        if op == OP_FUNCTION && !declared {
            out.extend_from_slice(&decl);
            declared = true;
        }
        if op == OP_FUNCTION {
            in_entry = a[1] == entry;
        }
        match op {
            OP_ENTRY_POINT if version >= 0x0001_0400 => {
                // SPIR-V 1.4+: every global the entry point uses is in its interface
                let mut w = whole.to_vec();
                w.extend(outs.iter().map(|o| shadow[&o.0]));
                w[0] = ((w.len() as u32) << 16) | op;
                out.extend(w);
            }
            OP_ACCESS_CHAIN | OP_IN_BOUNDS_ACCESS_CHAIN if is_out(a[2]) => {
                let c = ptr[&a[0]].1;
                let mut w = whole.to_vec();
                w[1] = priv_ptr[&(c | 0x8000_0000)];
                w[3] = shadow[&a[2]];
                out.extend(w);
            }
            OP_LOAD if is_out(a[2]) => {
                let mut w = whole.to_vec();
                w[3] = shadow[&a[2]];
                out.extend(w);
            }
            OP_STORE if is_out(a[0]) => {
                let mut w = whole.to_vec();
                w[1] = shadow[&a[0]];
                out.extend(w);
            }
            OP_COPY_MEMORY if is_out(a[0]) || is_out(a[1]) => return None,
            OP_RETURN if in_entry => {
                for &(var, t) in &outs {
                    let tmp = new_id();
                    out.extend(ins(OP_LOAD, &[t, tmp, shadow[&var]]));
                    out.extend(ins(OP_STORE, &[var, tmp]));
                }
                out.extend_from_slice(whole);
            }
            _ => {
                // any other instruction in a function naming an output: not rewritten (the module stays as it was)
                let ids: &[u32] = match op {
                    // literals that may equal an id: an extended instruction's number, merge controls, switch cases
                    OP_EXT_INST => &a[..3.min(a.len())],
                    OP_SELECTION_MERGE | OP_LOOP_MERGE | OP_SWITCH | OP_LINE => &[],
                    _ => a,
                };
                if declared && op != OP_NAME && op != OP_DECORATE && op != OP_VARIABLE && ids.iter().any(|&w| is_out(w)) {
                    return None;
                }
                if op == OP_EXT_INST && a.len() > 4 && a[4..].iter().any(|&w| is_out(w)) {
                    return None;
                }
                out.extend_from_slice(whole);
            }
        }
    }
    out[3] = bound;
    Some(out)
}
