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

/// `sabs_product`: every 32-bit GLSL.std.450 SAbs written as `x * ((x >> 31) | 1)` (mkw-recomp's translator, #395 /
/// #22: NVIDIA's compiler turns an integer abs of a small select followed by an add or xor into code that faults the GPU
/// with "instruction pointer invalid", device lost; it reads `x < 0 ? -x : x`, `max(x, -x)` and `(x ^ m) - m` as that
/// abs too, and leaves the product alone; bit-exact for every 32-bit input). Mario Kart World 1.8.0 lost the device on
/// the title screen 30 s in (lane K; 1,451 of its 11,609 shaders use SAbs). None when the module has none.
pub fn sabs_product(words: &[u32]) -> Option<Vec<u32>> {
    const OP_EXT_INST_IMPORT: u32 = 11;
    const OP_TYPE_INT: u32 = 21;
    const OP_TYPE_VECTOR: u32 = 23;
    const OP_CONSTANT: u32 = 43;
    const OP_CONSTANT_COMPOSITE: u32 = 44;
    const OP_IMUL: u32 = 132;
    const OP_SHIFT_RIGHT_ARITHMETIC: u32 = 195;
    const OP_BITWISE_OR: u32 = 197;
    const GLSL_SABS: u32 = 5;
    if words.len() < 5 || words[0] != 0x0723_0203 {
        return None;
    }
    // (offset, opcode, word count) of every instruction
    let mut v = vec![];
    let mut i = 5;
    while i < words.len() {
        let n = (words[i] >> 16) as usize;
        if n == 0 || i + n > words.len() {
            return None;
        }
        v.push((i, words[i] & 0xffff, n));
        i += n;
    }
    let mut glsl = vec![];
    // 32-bit int types: id -> (component count, scalar type id)
    let mut ints: HashMap<u32, (u32, u32)> = HashMap::new();
    let mut first_fn = 0;
    let mut sites = 0;
    for &(i, op, n) in &v {
        match op {
            OP_EXT_INST_IMPORT if n >= 3 => {
                let name: Vec<u8> = words[i + 2..i + n].iter().flat_map(|w| w.to_le_bytes()).take_while(|&b| b != 0).collect();
                if name == b"GLSL.std.450" {
                    glsl.push(words[i + 1]);
                }
            }
            OP_TYPE_INT if n == 4 && words[i + 2] == 32 => {
                ints.insert(words[i + 1], (1, words[i + 1]));
            }
            OP_TYPE_VECTOR if n == 4 => {
                if let Some(&(1, s)) = ints.get(&words[i + 2]) {
                    ints.insert(words[i + 1], (words[i + 3], s));
                }
            }
            OP_FUNCTION if first_fn == 0 => first_fn = i,
            OP_EXT_INST if n == 6 && glsl.contains(&words[i + 3]) && words[i + 4] == GLSL_SABS && ints.contains_key(&words[i + 1]) => {
                sites += 1;
            }
            _ => {}
        }
    }
    if sites == 0 || first_fn == 0 {
        return None;
    }
    let mut bound = words[3];
    let mut fresh = || {
        bound += 1;
        bound - 1
    };
    // the constants 31 and 1 of each type an SAbs returns, declared before the first function
    let mut consts: HashMap<u32, (u32, u32)> = HashMap::new();
    let mut decl = vec![];
    let mut scalar: HashMap<u32, (u32, u32)> = HashMap::new();
    for &(i, op, n) in &v {
        if op != OP_EXT_INST || n != 6 || !glsl.contains(&words[i + 3]) || words[i + 4] != GLSL_SABS {
            continue;
        }
        let t = words[i + 1];
        let Some(&(count, s)) = ints.get(&t) else { continue };
        if consts.contains_key(&t) {
            continue;
        }
        let (s31, s1) = *scalar.entry(s).or_insert_with(|| {
            let (a, b) = (fresh(), fresh());
            decl.extend(ins(OP_CONSTANT, &[s, a, 31]));
            decl.extend(ins(OP_CONSTANT, &[s, b, 1]));
            (a, b)
        });
        if count == 1 {
            consts.insert(t, (s31, s1));
        } else {
            let (a, b) = (fresh(), fresh());
            let mut o31 = vec![t, a];
            let mut o1 = vec![t, b];
            for _ in 0..count {
                o31.push(s31);
                o1.push(s1);
            }
            decl.extend(ins(OP_CONSTANT_COMPOSITE, &o31));
            decl.extend(ins(OP_CONSTANT_COMPOSITE, &o1));
            consts.insert(t, (a, b));
        }
    }
    let mut out = Vec::with_capacity(words.len() + decl.len() + sites * 9);
    out.extend_from_slice(&words[..5]);
    for &(i, op, n) in &v {
        if i == first_fn {
            out.extend_from_slice(&decl);
        }
        if op == OP_EXT_INST && n == 6 && glsl.contains(&words[i + 3]) && words[i + 4] == GLSL_SABS {
            if let Some(&(c31, c1)) = consts.get(&words[i + 1]) {
                let (t, r, x) = (words[i + 1], words[i + 2], words[i + 5]);
                let (sh, or) = (fresh(), fresh());
                out.extend(ins(OP_SHIFT_RIGHT_ARITHMETIC, &[t, sh, x, c31]));
                out.extend(ins(OP_BITWISE_OR, &[t, or, sh, c1]));
                out.extend(ins(OP_IMUL, &[t, r, x, or]));
                continue;
            }
        }
        out.extend_from_slice(&words[i..i + n]);
    }
    out[3] = bound;
    Some(out)
}

#[cfg(test)]
mod sabs_tests {
    use super::*;

    /// A module with `%r = SAbs %x` on an int and on an ivec2 comes out with the product and its constants, and the
    /// product gives abs for every sign (checked on the formula the words encode).
    #[test]
    fn sabs_becomes_a_product() {
        let mut w = vec![0x0723_0203, 0x0001_0000, 0, 100, 0];
        w.extend(ins(11, &[1, u32::from_le_bytes(*b"GLSL"), u32::from_le_bytes(*b".std"), u32::from_le_bytes(*b".450"), 0]));
        w.extend(ins(21, &[2, 32, 1])); // int
        w.extend(ins(23, &[3, 2, 2])); // ivec2
        w.extend(ins(OP_FUNCTION, &[2, 10, 0, 11]));
        w.extend(ins(OP_EXT_INST, &[2, 20, 1, 5, 21]));
        w.extend(ins(OP_EXT_INST, &[3, 22, 1, 5, 23]));
        w.extend(ins(OP_EXT_INST, &[2, 24, 1, 4, 21])); // FAbs-numbered other instruction stays
        let out = sabs_product(&w).unwrap();
        let ops: Vec<u32> = {
            let mut v = vec![];
            let mut i = 5;
            while i < out.len() {
                v.push(out[i] & 0xffff);
                i += (out[i] >> 16) as usize;
            }
            v
        };
        // import, int, ivec2, 31, 1, ivec2(31), ivec2(1), function, 2 x (shift, or, mul), the other ext inst
        assert_eq!(ops, [11, 21, 23, 43, 43, 44, 44, 54, 195, 197, 132, 195, 197, 132, 12]);
        assert_eq!(out[3], 100 + 4 + 4);
        assert!(sabs_product(&out).is_none(), "nothing left to rewrite");
        for x in [0i32, 1, -1, 7, -7, i32::MAX, i32::MIN, -2_000_000_000] {
            assert_eq!(x.wrapping_mul((x >> 31) | 1), x.wrapping_abs());
        }
    }
}
