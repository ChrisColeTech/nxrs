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
                Some(&m) if !phi_at.get(&m).copied().unwrap_or(true) => out.extend(ins(OP_BRANCH, &[m])),
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
            let mut ws: Vec<u32> = name.chunks(4).map(|c| {
                let mut b = [0u8; 4];
                b[..c.len()].copy_from_slice(c);
                u32::from_le_bytes(b)
            }).collect();
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
        assert_eq!(ops(&out), [17, 17, 10, 19, 54, 248, 247, 250, 248, 5380, 249, 248, 253, 56]);
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
    let (Ok(i), Ok(o)) = (std::env::var("NXGFX_SPVFIX_IN"), std::env::var("NXGFX_SPVFIX_OUT")) else { return };
    std::fs::create_dir_all(&o).unwrap();
    let mut n = 0;
    for e in std::fs::read_dir(&i).unwrap().flatten() {
        let b = std::fs::read(e.path()).unwrap();
        if b.len() % 4 != 0 {
            continue;
        }
        let w: Vec<u32> = b.chunks(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
        let k = kill_demote(&w);
        let s = sabs_product(k.as_deref().unwrap_or(&w));
        if k.is_none() && s.is_none() {
            continue;
        }
        let out = s.or(k).unwrap();
        let bytes: Vec<u8> = out.iter().flat_map(|x| x.to_le_bytes()).collect();
        std::fs::write(std::path::Path::new(&o).join(e.file_name()), bytes).unwrap();
        n += 1;
    }
    eprintln!("{n} modules rewritten");
}
