//! Shape changes to translated SPIR-V a driver's compiler needs (the program's meaning is unchanged).
//!
//! `late_outputs`: a vertex shader's Location outputs are written to private copies wherever the translated code
//! writes them and stored to the outputs once, before each return of the entry point. The translator writes each
//! output component where the guest program does, often inside branches; NVIDIA's compiler (nvgpucomp64) faulted
//! compiling Donkey Kong Bananza's vertex shader d87ccbede95b9f2c that way (as a pipeline-library part with any pixel
//! shader, and whole with pixel shader 6655320127e098d6), and compiles it with its outputs written once at the end.

use std::collections::HashMap;

/// The device has shaderDemoteToHelperInvocation (set when the device is made): `kill_demote` may run.
pub static DEMOTE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

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
    let mut components = HashMap::new();
    let mut types = HashMap::new();
    let mut initializers = HashMap::new();
    for x in &v {
        if x.1 == OP_DECORATE && x.2.len() >= 2 && x.2[1] == DEC_LOCATION {
            located.insert(x.2[0]);
        }
        if x.1 == OP_DECORATE && x.2.len() == 3 && x.2[1] == 31 {
            components.insert(x.2[0], x.2[2]);
        }
        if matches!(x.1, 21..=23) {
            types.insert(x.2[0], (x.1, &x.2[1..]));
        }
        if x.1 == OP_VARIABLE && x.2.len() == 4 && x.2[2] == SC_OUTPUT {
            initializers.insert(x.2[1], x.2[3]);
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
        if x.1 == OP_VARIABLE && x.2.len() >= 3 && x.2[2] == SC_OUTPUT && located.contains(&x.2[1])
        {
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
    // Preserve explicit initializers. Otherwise use the translator's unwritten user-attribute
    // defaults (0, 0, 0, 1), including Component decorations. A full-vector export must not turn
    // an unwritten W into zero: fullscreen lighting shaders divide by that varying's W.
    let mut decl: Vec<u32> = Vec::new();
    let mut priv_ptr: HashMap<u32, u32> = HashMap::new();
    let mut defaults: HashMap<(u32, u32), u32> = HashMap::new();
    let mut shadow: HashMap<u32, u32> = HashMap::new();
    for &(var, t) in &outs {
        let pp = *priv_ptr.entry(t).or_insert_with(|| {
            let id = new_id();
            decl.extend(ins(OP_TYPE_POINTER, &[id, SC_PRIVATE, t]));
            id
        });
        let component = components.get(&var).copied().unwrap_or(0);
        let init = if let Some(&init) = initializers.get(&var) {
            init
        } else if let Some(&init) = defaults.get(&(t, component)) {
            init
        } else {
            let (scalar, count) = match types.get(&t) {
                Some((23, a)) => (a[0], a[1]),
                _ => (t, 1),
            };
            let init = match types.get(&scalar) {
                Some((op @ (21 | 22), a)) if a[0] == 32 && count <= 4 => {
                    let mut values = Vec::new();
                    for c in 0..count {
                        let value = if component + c != 3 {
                            0
                        } else if *op == 22 {
                            1.0f32.to_bits()
                        } else {
                            1
                        };
                        let id = new_id();
                        decl.extend(ins(43, &[scalar, id, value]));
                        values.push(id);
                    }
                    if count == 1 {
                        values[0]
                    } else {
                        let id = new_id();
                        let mut args = vec![t, id];
                        args.extend(values);
                        decl.extend(ins(44, &args));
                        id
                    }
                }
                _ => {
                    let id = new_id();
                    decl.extend(ins(OP_CONSTANT_NULL, &[t, id]));
                    id
                }
            };
            defaults.insert((t, component), init);
            init
        };
        let s = new_id();
        decl.extend(ins(OP_VARIABLE, &[pp, s, SC_PRIVATE, init]));
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
                if declared
                    && op != OP_NAME
                    && op != OP_DECORATE
                    && op != OP_VARIABLE
                    && ids.iter().any(|&w| is_out(w))
                {
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

#[cfg(test)]
mod late_output_tests {
    use super::*;

    fn fixture(component: u32, count: u32, initializer: bool) -> Vec<u32> {
        let mut w = vec![0x0723_0203, 0x0001_0300, 0, 20, 0];
        w.extend(ins(OP_ENTRY_POINT, &[0, 10, 0x6e69616d, 0, 5]));
        w.extend(ins(OP_DECORATE, &[5, DEC_LOCATION, 0]));
        w.extend(ins(OP_DECORATE, &[5, 31, component]));
        w.extend(ins(22, &[1, 32]));
        w.extend(ins(23, &[2, 1, count]));
        w.extend(ins(OP_TYPE_POINTER, &[3, SC_OUTPUT, 2]));
        w.extend(ins(OP_CONSTANT_NULL, &[2, 4]));
        w.extend(ins(
            OP_VARIABLE,
            if initializer {
                &[3, 5, SC_OUTPUT, 4]
            } else {
                &[3, 5, SC_OUTPUT]
            },
        ));
        w.extend(ins(OP_FUNCTION, &[6, 10, 0, 7]));
        w.extend(ins(OP_RETURN, &[]));
        w
    }

    fn shadow_value(w: &[u32]) -> Vec<u32> {
        let mut constants: HashMap<u32, Vec<u32>> = HashMap::new();
        let mut at = 5;
        while at < w.len() {
            let n = (w[at] >> 16) as usize;
            let a = &w[at + 1..at + n];
            match w[at] & 65535 {
                43 => {
                    constants.insert(a[1], vec![a[2]]);
                }
                44 => {
                    constants.insert(a[1], a[2..].iter().map(|id| constants[id][0]).collect());
                }
                OP_VARIABLE if a[2] == SC_PRIVATE => return constants[&a[3]].clone(),
                _ => {}
            }
            at += n;
        }
        panic!("no private output");
    }

    #[test]
    fn late_output_defaults_keep_w_one_and_respect_component() {
        assert_eq!(
            shadow_value(&late_outputs(&fixture(0, 4, false)).unwrap()),
            [0, 0, 0, 1.0f32.to_bits()]
        );
        assert_eq!(
            shadow_value(&late_outputs(&fixture(2, 2, false)).unwrap()),
            [0, 1.0f32.to_bits()]
        );
        assert_eq!(
            shadow_value(&late_outputs(&fixture(0, 2, false)).unwrap()),
            [0, 0]
        );
    }

    #[test]
    fn late_outputs_keep_explicit_initializer() {
        let w = late_outputs(&fixture(0, 4, true)).unwrap();
        let mut at = 5;
        let mut found = false;
        while at < w.len() {
            let n = (w[at] >> 16) as usize;
            if w[at] & 65535 == OP_VARIABLE && w[at + 3] == SC_PRIVATE {
                assert_eq!(w[at + 4], 4, "explicit zero must override the default W");
                found = true;
            }
            at += n;
        }
        assert!(found);
    }
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
                let name: Vec<u8> = words[i + 2..i + n]
                    .iter()
                    .flat_map(|w| w.to_le_bytes())
                    .take_while(|&b| b != 0)
                    .collect();
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
            OP_EXT_INST
                if n == 6
                    && glsl.contains(&words[i + 3])
                    && words[i + 4] == GLSL_SABS
                    && ints.contains_key(&words[i + 1]) =>
            {
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
        if op != OP_EXT_INST || n != 6 || !glsl.contains(&words[i + 3]) || words[i + 4] != GLSL_SABS
        {
            continue;
        }
        let t = words[i + 1];
        let Some(&(count, s)) = ints.get(&t) else {
            continue;
        };
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
        if op == OP_EXT_INST && n == 6 && glsl.contains(&words[i + 3]) && words[i + 4] == GLSL_SABS
        {
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
        w.extend(ins(
            11,
            &[
                1,
                u32::from_le_bytes(*b"GLSL"),
                u32::from_le_bytes(*b".std"),
                u32::from_le_bytes(*b".450"),
                0,
            ],
        ));
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
        assert_eq!(
            ops,
            [11, 21, 23, 43, 43, 44, 44, 54, 195, 197, 132, 195, 197, 132, 12]
        );
        assert_eq!(out[3], 100 + 4 + 4);
        assert!(sabs_product(&out).is_none(), "nothing left to rewrite");
        for x in [0i32, 1, -1, 7, -7, i32::MAX, i32::MIN, -2_000_000_000] {
            assert_eq!(x.wrapping_mul((x >> 31) | 1), x.wrapping_abs());
        }
    }
}

/// `kill_demote`: every OpKill (a discard that ends the pixel's invocation midway) made OpDemoteToHelperInvocation, so
/// the pixel stays a helper to the end of the shader (mkw-recomp #22 / #231, as Switch emulators and DXVK do): on
/// NVIDIA a pass whose fragment shader discards midway stuck for 1-1.5 s and lost the device ("instruction pointer
/// invalid"). Mario Kart World 1.8.0 lost the device ~40 s in, on the title scene after the course map (lane K; 2,186
/// of its fragment shaders discard). The kill's block then branches to the merge block of the selection it ends (the
/// usual shape: a header whose OpBranchConditional reaches it), else returns (OpReturnValue of an OpUndef in a function
/// that returns a value). None when the module has no OpKill.
pub fn kill_demote(words: &[u32]) -> Option<Vec<u32>> {
    const OP_EXTENSION: u32 = 10;
    const OP_CAPABILITY: u32 = 17;
    const OP_TYPE_VOID: u32 = 19;
    const OP_FUNCTION_END: u32 = 56;
    const OP_UNDEF: u32 = 1;
    const OP_PHI: u32 = 245;
    const OP_LABEL: u32 = 248;
    const OP_BRANCH: u32 = 249;
    const OP_BRANCH_CONDITIONAL: u32 = 250;
    const OP_KILL: u32 = 252;
    const OP_RETURN_VALUE: u32 = 254;
    const OP_DEMOTE: u32 = 5380;
    const CAP_DEMOTE: u32 = 5379;
    if words.len() < 5 || words[0] != 0x0723_0203 {
        return None;
    }
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
    if !v.iter().any(|&(_, op, _)| op == OP_KILL) {
        return None;
    }
    let mut voids = vec![];
    let mut has_cap = false;
    let mut last_cap = 0;
    // label -> the merge block of the selection whose header branches to it; label -> starts with OpPhi
    let mut merge_of: HashMap<u32, u32> = HashMap::new();
    let mut phi_at: HashMap<u32, bool> = HashMap::new();
    let mut pending_merge = 0;
    let mut label = 0;
    for (k, &(i, op, n)) in v.iter().enumerate() {
        match op {
            OP_CAPABILITY if n == 2 => {
                last_cap = k;
                has_cap |= words[i + 1] == CAP_DEMOTE;
            }
            OP_TYPE_VOID if n == 2 => voids.push(words[i + 1]),
            OP_LABEL if n == 2 => {
                label = words[i + 1];
                pending_merge = 0;
                let first = v.get(k + 1).map(|x| x.1);
                phi_at.insert(label, first == Some(OP_PHI));
            }
            OP_SELECTION_MERGE if n >= 2 => pending_merge = words[i + 1],
            OP_BRANCH_CONDITIONAL if n >= 4 && pending_merge != 0 => {
                for t in [words[i + 2], words[i + 3]] {
                    if t != pending_merge {
                        merge_of.entry(t).or_insert(pending_merge);
                    }
                }
            }
            _ => {}
        }
        let _ = label;
    }
    let mut bound = words[3];
    let mut out = Vec::with_capacity(words.len() + 64);
    out.extend_from_slice(&words[..5]);
    let (mut fn_type, mut block) = (0u32, 0u32);
    for (k, &(i, op, n)) in v.iter().enumerate() {
        if op == OP_FUNCTION {
            fn_type = words[i + 1];
        }
        if op == OP_FUNCTION_END {
            fn_type = 0;
        }
        if op == OP_LABEL {
            block = words[i + 1];
        }
        if op == OP_KILL {
            out.extend(ins(OP_DEMOTE, &[]));
            match merge_of.get(&block) {
                Some(&m) if !phi_at.get(&m).copied().unwrap_or(true) => {
                    out.extend(ins(OP_BRANCH, &[m]))
                }
                _ if voids.contains(&fn_type) => out.extend(ins(OP_RETURN, &[])),
                _ => {
                    let u = bound;
                    bound += 1;
                    out.extend(ins(OP_UNDEF, &[fn_type, u]));
                    out.extend(ins(OP_RETURN_VALUE, &[u]));
                }
            }
            continue;
        }
        out.extend_from_slice(&words[i..i + n]);
        if k == last_cap && !has_cap {
            out.extend(ins(OP_CAPABILITY, &[CAP_DEMOTE]));
            let name = b"SPV_EXT_demote_to_helper_invocation\0";
            let mut ws: Vec<u32> = name
                .chunks(4)
                .map(|c| {
                    let mut b = [0u8; 4];
                    b[..c.len()].copy_from_slice(c);
                    u32::from_le_bytes(b)
                })
                .collect();
            out.extend(ins(OP_EXTENSION, &ws));
        }
    }
    out[3] = bound;
    Some(out)
}

#[cfg(test)]
mod kill_tests {
    use super::*;

    fn ops(w: &[u32]) -> Vec<u32> {
        let mut v = vec![];
        let mut i = 5;
        while i < w.len() {
            v.push(w[i] & 0xffff);
            i += (w[i] >> 16) as usize;
        }
        v
    }

    /// `if (c) discard;` in main: demote, then on to the selection's merge block; the capability and extension added.
    #[test]
    fn kill_in_a_selection_branches_to_its_merge() {
        let mut w = vec![0x0723_0203, 0x0001_0300, 0, 50, 0];
        w.extend(ins(17, &[1])); // Shader
        w.extend(ins(19, &[2])); // void
        w.extend(ins(OP_FUNCTION, &[2, 10, 0, 11]));
        w.extend(ins(248, &[12]));
        w.extend(ins(OP_SELECTION_MERGE, &[14, 0]));
        w.extend(ins(250, &[20, 13, 14]));
        w.extend(ins(248, &[13]));
        w.extend(ins(252, &[]));
        w.extend(ins(248, &[14]));
        w.extend(ins(OP_RETURN, &[]));
        w.extend(ins(56, &[]));
        let out = kill_demote(&w).unwrap();
        assert_eq!(
            ops(&out),
            [17, 17, 10, 19, 54, 248, 247, 250, 248, 5380, 249, 248, 253, 56]
        );
        assert!(kill_demote(&out).is_none());
        // the branch goes to the merge block 14
        let at = out.iter().position(|&x| x & 0xffff == 5380).unwrap();
        assert_eq!(out[at + 2], 14);
    }

    /// A kill with no selection around it in a function returning a value: demote and return an undefined value.
    #[test]
    fn kill_elsewhere_returns() {
        let mut w = vec![0x0723_0203, 0x0001_0300, 0, 50, 0];
        w.extend(ins(17, &[1]));
        w.extend(ins(21, &[3, 32, 1])); // int
        w.extend(ins(OP_FUNCTION, &[3, 10, 0, 11]));
        w.extend(ins(248, &[12]));
        w.extend(ins(252, &[]));
        w.extend(ins(56, &[]));
        let out = kill_demote(&w).unwrap();
        assert_eq!(ops(&out), [17, 17, 10, 21, 54, 248, 5380, 1, 254, 56]);
        assert_eq!(out[3], 51);
    }
}

/// The two rewrites over a folder of translated modules (`NXGFX_SPVFIX_IN`), written to `NXGFX_SPVFIX_OUT` for
/// spirv-val; opt-in: `cargo test -p graphics --lib spvfix_folder -- --ignored`.
#[cfg(test)]
#[test]
#[ignore]
fn spvfix_folder() {
    let (Ok(i), Ok(o)) = (
        std::env::var("NXGFX_SPVFIX_IN"),
        std::env::var("NXGFX_SPVFIX_OUT"),
    ) else {
        return;
    };
    std::fs::create_dir_all(&o).unwrap();
    let mut n = 0;
    for e in std::fs::read_dir(&i).unwrap().flatten() {
        let b = std::fs::read(e.path()).unwrap();
        if b.len() % 4 != 0 {
            continue;
        }
        let w: Vec<u32> = b
            .chunks(4)
            .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
            .collect();
        let k = kill_demote(&w);
        let s = sabs_product(k.as_deref().unwrap_or(&w));
        let m = mesh_indices(s.as_deref().or(k.as_deref()).unwrap_or(&w));
        // shared memory zeroed: task / mesh shaders only (as shader.rs applies it)
        let is_mesh = {
            let (mut i, mut f) = (5, false);
            while i < w.len() && w[i] >> 16 != 0 {
                f |= w[i] & 0xffff == OP_ENTRY_POINT && (w[i + 1] == 5364 || w[i + 1] == 5365);
                i += (w[i] >> 16) as usize;
            }
            f
        };
        let z = if is_mesh {
            zero_workgroup(m.as_deref().or(s.as_deref()).or(k.as_deref()).unwrap_or(&w))
        } else {
            None
        };
        if k.is_none() && s.is_none() && m.is_none() && z.is_none() {
            continue;
        }
        let out = z.or(m).or(s).or(k).unwrap();
        let bytes: Vec<u8> = out.iter().flat_map(|x| x.to_le_bytes()).collect();
        std::fs::write(std::path::Path::new(&o).join(e.file_name()), bytes).unwrap();
        n += 1;
    }
    eprintln!("{n} modules rewritten");
}

/// `mesh_indices`: a mesh shader's primitive indices held below its vertex count (each stored index AND
/// (OutputVertices - 1); the vertex count a power of two, as the translator declares it). The translated mesh shaders
/// take each primitive's three indices from shared memory, eight bits each (0..255), against 32 vertices declared, and
/// a primitive whose indices the game did not write there (shared memory starts undefined) named a vertex past the
/// outputs; mkw-recomp's translator zeroes its index array first and its count stays inside it. Mario Kart World lost
/// the device in direct mesh draws with an "invalid read" while indirect / 3D ones were skipped (lane K2). Indices
/// already in range are unchanged. None when the module stores no primitive indices.
pub fn mesh_indices(words: &[u32]) -> Option<Vec<u32>> {
    const OP_EXECUTION_MODE: u32 = 16;
    const OP_TYPE_INT: u32 = 21;
    const OP_TYPE_VECTOR: u32 = 23;
    const OP_CONSTANT: u32 = 43;
    const OP_CONSTANT_COMPOSITE: u32 = 44;
    const OP_BITWISE_AND: u32 = 199;
    const MODE_OUTPUT_VERTICES: u32 = 26;
    const DEC_BUILTIN: u32 = 11;
    // PrimitivePointIndicesEXT, PrimitiveLineIndicesEXT, PrimitiveTriangleIndicesEXT
    const INDEX_BUILTINS: [u32; 3] = [5294, 5295, 5296];
    if words.len() < 5 || words[0] != 0x0723_0203 {
        return None;
    }
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
    let mut verts = 0u32;
    let mut index_vars: Vec<u32> = vec![];
    let mut uints: HashMap<u32, ()> = HashMap::new();
    let mut vecs: HashMap<u32, (u32, u32)> = HashMap::new();
    let mut ptrs: HashMap<u32, u32> = HashMap::new();
    let mut first_fn = 0;
    for &(i, op, n) in &v {
        match op {
            OP_EXECUTION_MODE if n >= 4 && words[i + 2] == MODE_OUTPUT_VERTICES => {
                verts = words[i + 3]
            }
            OP_DECORATE
                if n >= 4
                    && words[i + 2] == DEC_BUILTIN
                    && INDEX_BUILTINS.contains(&words[i + 3]) =>
            {
                index_vars.push(words[i + 1])
            }
            OP_TYPE_INT if n == 4 && words[i + 2] == 32 && words[i + 3] == 0 => {
                uints.insert(words[i + 1], ());
            }
            OP_TYPE_VECTOR if n == 4 => {
                vecs.insert(words[i + 1], (words[i + 2], words[i + 3]));
            }
            OP_TYPE_POINTER if n == 4 => {
                ptrs.insert(words[i + 1], words[i + 3]);
            }
            OP_FUNCTION if first_fn == 0 => first_fn = i,
            _ => {}
        }
    }
    if index_vars.is_empty() || verts == 0 || !verts.is_power_of_two() || first_fn == 0 {
        return None;
    }
    // access chains into the index arrays: result id -> pointee type
    let mut chains: HashMap<u32, u32> = HashMap::new();
    for &(i, op, n) in &v {
        if (op == OP_ACCESS_CHAIN || op == OP_IN_BOUNDS_ACCESS_CHAIN)
            && n >= 4
            && index_vars.contains(&words[i + 3])
        {
            chains.insert(words[i + 2], *ptrs.get(&words[i + 1])?);
        }
    }
    let mut bound = words[3];
    let mut fresh = || {
        bound += 1;
        bound - 1
    };
    let mut consts: HashMap<u32, u32> = HashMap::new();
    let mut scalar: HashMap<u32, u32> = HashMap::new();
    let mut decl = vec![];
    let mut sites = 0;
    for &(i, op, n) in &v {
        if op != OP_STORE || n < 3 {
            continue;
        }
        if index_vars.contains(&words[i + 1]) {
            return None; // the whole array stored at once: not rewritten
        }
        let Some(&t) = chains.get(&words[i + 1]) else {
            continue;
        };
        let s = if uints.contains_key(&t) {
            t
        } else {
            vecs.get(&t)
                .filter(|(c, _)| uints.contains_key(c))
                .map(|(c, _)| *c)?
        };
        if !consts.contains_key(&t) {
            let sc = *scalar.entry(s).or_insert_with(|| {
                let c = fresh();
                decl.extend(ins(OP_CONSTANT, &[s, c, verts - 1]));
                c
            });
            let c = if t == s {
                sc
            } else {
                let c = fresh();
                let mut o = vec![t, c];
                o.extend(std::iter::repeat_n(sc, vecs[&t].1 as usize));
                decl.extend(ins(OP_CONSTANT_COMPOSITE, &o));
                c
            };
            consts.insert(t, c);
        }
        sites += 1;
    }
    if sites == 0 {
        return None;
    }
    let mut out = Vec::with_capacity(words.len() + decl.len() + sites * 5);
    out.extend_from_slice(&words[..5]);
    for &(i, op, n) in &v {
        if i == first_fn {
            out.extend_from_slice(&decl);
        }
        if op == OP_STORE && n >= 3 {
            if let Some(t) = chains.get(&words[i + 1]) {
                let r = fresh();
                out.extend(ins(OP_BITWISE_AND, &[*t, r, words[i + 2], consts[t]]));
                let mut st = words[i..i + n].to_vec();
                st[2] = r;
                out.extend(st);
                continue;
            }
        }
        out.extend_from_slice(&words[i..i + n]);
    }
    out[3] = bound;
    Some(out)
}

#[cfg(test)]
mod mesh_index_tests {
    use super::*;

    /// A store of a uvec3 into gl_PrimitiveTriangleIndicesEXT[i] gets an AND with uvec3(31) first (32 vertices).
    #[test]
    fn primitive_indices_are_held_below_the_vertex_count() {
        let mut w = vec![0x0723_0203, 0x0001_0000, 0, 100, 0];
        w.extend(ins(16, &[1, 26, 32])); // OutputVertices 32
        w.extend(ins(OP_DECORATE, &[9, 11, 5296]));
        w.extend(ins(21, &[2, 32, 0])); // uint
        w.extend(ins(23, &[3, 2, 3])); // uvec3
        w.extend(ins(OP_TYPE_POINTER, &[4, 3, 3])); // Output uvec3*
        w.extend(ins(OP_FUNCTION, &[2, 10, 0, 11]));
        w.extend(ins(OP_ACCESS_CHAIN, &[4, 20, 9, 21]));
        w.extend(ins(OP_STORE, &[20, 22]));
        w.extend(ins(OP_STORE, &[30, 22])); // another store stays
        let out = mesh_indices(&w).unwrap();
        let ops: Vec<u32> = {
            let mut v = vec![];
            let mut i = 5;
            while i < out.len() {
                v.push(out[i] & 0xffff);
                i += (out[i] >> 16) as usize;
            }
            v
        };
        // mode, decorate, uint, uvec3, pointer, 31, uvec3(31), function, chain, and, store, store
        assert_eq!(ops, [16, 71, 21, 23, 32, 43, 44, 54, 65, 199, 62, 62]);
        assert!(out.windows(3).any(|x| x == [2, 100, 31]), "the constant 31");
        assert_eq!(out[3], 103);
    }
}

/// The device has shaderZeroInitializeWorkgroupMemory (set when the device is made): `zero_workgroup` may run.
pub static ZERO_WG: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `zero_workgroup`: every Workgroup variable (shared memory, task payload staging) without an initializer gets a
/// null one, so a word the program reads before writing it reads 0, not what the last workgroup on that SM left there.
/// The translated task / mesh shaders take counts and indices from shared memory (a task's mesh-group count at word
/// 357, a mesh shader's primitive count and indices); mkw-recomp's translator zeroes its index array before a mesh
/// shader runs. None when the module has no such variable.
pub fn zero_workgroup(words: &[u32]) -> Option<Vec<u32>> {
    const SC_WORKGROUP: u32 = 4;
    if words.len() < 5 || words[0] != 0x0723_0203 {
        return None;
    }
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
    let mut ptrs: HashMap<u32, u32> = HashMap::new();
    let mut sites = 0;
    for &(i, op, n) in &v {
        if op == OP_TYPE_POINTER && n == 4 {
            ptrs.insert(words[i + 1], words[i + 3]);
        }
        if op == OP_FUNCTION {
            break;
        }
        if op == OP_VARIABLE && n == 4 && words[i + 3] == SC_WORKGROUP {
            sites += 1;
        }
    }
    if sites == 0 {
        return None;
    }
    let mut bound = words[3];
    let mut out = Vec::with_capacity(words.len() + sites * 8);
    out.extend_from_slice(&words[..5]);
    let mut in_fn = false;
    for &(i, op, n) in &v {
        in_fn |= op == OP_FUNCTION;
        if !in_fn && op == OP_VARIABLE && n == 4 && words[i + 3] == SC_WORKGROUP {
            if let Some(&t) = ptrs.get(&words[i + 1]) {
                let z = bound;
                bound += 1;
                out.extend(ins(OP_CONSTANT_NULL, &[t, z]));
                out.extend(ins(
                    OP_VARIABLE,
                    &[words[i + 1], words[i + 2], SC_WORKGROUP, z],
                ));
                continue;
            }
        }
        out.extend_from_slice(&words[i..i + n]);
    }
    out[3] = bound;
    Some(out)
}

#[cfg(test)]
mod zero_wg_tests {
    use super::*;

    #[test]
    fn workgroup_variables_get_a_null_initializer() {
        let mut w = vec![0x0723_0203, 0x0001_0000, 0, 50, 0];
        w.extend(ins(21, &[2, 32, 0]));
        w.extend(ins(OP_TYPE_POINTER, &[3, 4, 2])); // Workgroup uint*
        w.extend(ins(OP_TYPE_POINTER, &[4, 3, 2])); // Output uint*
        w.extend(ins(OP_VARIABLE, &[3, 10, 4]));
        w.extend(ins(OP_VARIABLE, &[4, 11, 3]));
        w.extend(ins(OP_FUNCTION, &[2, 12, 0, 13]));
        let out = zero_workgroup(&w).unwrap();
        assert!(
            out.windows(4)
                .any(|x| x == [(3 << 16) | 46, 2, 50, (5 << 16) | 59]),
            "null constant before the variable"
        );
        assert!(
            out.windows(4).any(|x| x == [3, 10, 4, 50]),
            "the variable initialized with it"
        );
        assert!(
            out.windows(3).any(|x| x == [4, 11, 3]),
            "the output variable untouched"
        );
        assert_eq!(out[3], 51);
    }
}
