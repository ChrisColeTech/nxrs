//! Edits made when a translated module meets the device and its partner stage (graphics applies them at load and at
//! pipeline creation; built modules on disk stay as translated).
//!
//! - `trim_cull`: the translator's gl_PerVertex declares ClipDistance[8] and CullDistance[1] (Maxwell has 8 clip
//!   distances), more than a device with maxCombinedClipAndCullDistances 8 takes. The cull member goes when nothing
//!   reaches it: it is the block's last member, no access chain names it, the block is never loaded, stored or built
//!   whole (Turbo's `vkspv_trim_cull`).
//! - `io_locations` / `add_outputs`: a pixel shader may read a varying its vertex shader never writes (undefined on the
//!   console; Vulkan requires the previous stage to declare it). The vertex shader of such a pair gets an output at
//!   each missing location, of the type the pixel shader reads, holding (0, 0, 0, 1) as the translator's own unwritten
//!   outputs do.

const OP_ENTRY_POINT: u32 = 15;
const OP_TYPE_INT: u32 = 21;
const OP_TYPE_FLOAT: u32 = 22;
const OP_TYPE_VECTOR: u32 = 23;
const OP_TYPE_ARRAY: u32 = 28;
const OP_TYPE_RUNTIME_ARRAY: u32 = 29;
const OP_TYPE_STRUCT: u32 = 30;
const OP_TYPE_POINTER: u32 = 32;
const OP_CONSTANT: u32 = 43;
const OP_CONSTANT_COMPOSITE: u32 = 44;
const OP_FUNCTION: u32 = 54;
const OP_VARIABLE: u32 = 59;
const OP_LOAD: u32 = 61;
const OP_STORE: u32 = 62;
const OP_COPY_MEMORY: u32 = 63;
const OP_ACCESS_CHAIN: u32 = 65;
const OP_IN_BOUNDS_ACCESS_CHAIN: u32 = 66;
const OP_DECORATE: u32 = 71;
const OP_MEMBER_DECORATE: u32 = 72;
const OP_MEMBER_NAME: u32 = 6;
const OP_COMPOSITE_CONSTRUCT: u32 = 80;
const OP_COPY_OBJECT: u32 = 83;
const OP_BITCAST: u32 = 124;
const DEC_BUILTIN: u32 = 11;
const DEC_LOCATION: u32 = 30;
const DEC_COMPONENT: u32 = 31;
const DEC_INDEX: u32 = 32;
const BI_CLIP: u32 = 3;
const BI_CULL: u32 = 4;
const SC_INPUT: u32 = 1;
const SC_OUTPUT: u32 = 3;

fn ins(op: u32, ops: &[u32]) -> Vec<u32> {
    let mut v = Vec::with_capacity(ops.len() + 1);
    v.push(((ops.len() as u32 + 1) << 16) | op);
    v.extend_from_slice(ops);
    v
}

/// The instructions of a module as (start, opcode, length); None when it is not well-formed SPIR-V.
fn walk(w: &[u32]) -> Option<Vec<(usize, u32, usize)>> {
    if w.len() < 5 || w[0] != 0x0723_0203 {
        return None;
    }
    let mut v = Vec::new();
    let mut i = 5;
    while i < w.len() {
        let n = (w[i] >> 16) as usize;
        if n == 0 || i + n > w.len() {
            return None;
        }
        v.push((i, w[i] & 0xffff, n));
        i += n;
    }
    Some(v)
}

/// `words` without gl_PerVertex's cull member when the clip and cull arrays together pass `limit` and nothing reaches
/// it. `None`: nothing to do (or it cannot be done: the cull distances are used; the caller logs it with `needs_trim`).
pub fn trim_cull(w: &[u32], limit: u32) -> Option<Vec<u32>> {
    let v = walk(w)?;
    let bound = w[3] as usize;
    let (mut st, mut clip_m, mut cull_m) = (0u32, u32::MAX, u32::MAX);
    for &(i, op, n) in &v {
        if op == OP_MEMBER_DECORATE && n >= 5 && w[i + 3] == DEC_BUILTIN && (w[i + 4] == BI_CLIP || w[i + 4] == BI_CULL) {
            if st != 0 && st != w[i + 1] {
                return None; // more than one block declares them: left alone
            }
            st = w[i + 1];
            if w[i + 4] == BI_CLIP {
                clip_m = w[i + 2];
            } else {
                cull_m = w[i + 2];
            }
        }
    }
    if st == 0 || clip_m == u32::MAX || cull_m == u32::MAX || st as usize >= bound {
        return None;
    }
    let mut def = vec![0usize; bound];
    let mut cval = vec![0u32; bound];
    let mut isc = vec![false; bound];
    let mut si = 0usize;
    for &(i, op, n) in &v {
        if (OP_TYPE_INT..=OP_TYPE_POINTER).contains(&op) && n >= 2 && (w[i + 1] as usize) < bound {
            def[w[i + 1] as usize] = i;
        }
        if op == OP_TYPE_STRUCT && w[i + 1] == st {
            si = i;
        }
        if op == OP_CONSTANT && n >= 4 && (w[i + 2] as usize) < bound {
            cval[w[i + 2] as usize] = w[i + 3];
            isc[w[i + 2] as usize] = true;
        }
    }
    if si == 0 {
        return None;
    }
    let nmem = (w[si] >> 16) - 2;
    let len_of = |m: u32| -> u32 {
        if m >= nmem {
            return 0;
        }
        let t = w[si + 2 + m as usize] as usize;
        if t < bound && def[t] != 0 && w[def[t]] & 0xffff == OP_TYPE_ARRAY {
            let c = w[def[t] + 3] as usize;
            if c < bound && isc[c] {
                return cval[c];
            }
        }
        0
    };
    let (lc, lu) = (len_of(clip_m), len_of(cull_m));
    if lc == 0 || lu == 0 || lc + lu <= limit {
        return None;
    }
    let mut ok = cull_m == nmem - 1 && lc <= limit;
    // what refers to the block: arrays of it (2), pointers to those, the variables of those
    let mut blk = vec![0u8; bound];
    let mut bptr = vec![0u8; bound];
    let mut bvar = vec![0u8; bound];
    blk[st as usize] = 1;
    for &(i, op, n) in &v {
        let at = |k: usize| w[i + k] as usize;
        if (op == OP_TYPE_ARRAY || op == OP_TYPE_RUNTIME_ARRAY) && n >= 3 && at(2) < bound && blk[at(2)] != 0 && at(1) < bound {
            blk[at(1)] = 2;
        }
        if op == OP_TYPE_POINTER && n >= 4 && at(3) < bound && blk[at(3)] != 0 && at(1) < bound {
            bptr[at(1)] = blk[at(3)];
        }
        if op == OP_VARIABLE && n >= 4 && at(1) < bound && bptr[at(1)] != 0 && at(2) < bound {
            bvar[at(2)] = bptr[at(1)];
        }
    }
    for &(i, op, n) in &v {
        if !ok {
            break;
        }
        let at = |k: usize| w[i + k] as usize;
        if (op == OP_ACCESS_CHAIN || op == OP_IN_BOUNDS_ACCESS_CHAIN) && n >= 5 && at(3) < bound && bvar[at(3)] != 0 {
            let k = if bvar[at(3)] == 2 { 5 } else { 4 }; // the member index follows the vertex index of an arrayed block
            if k < n && (at(k) >= bound || !isc[at(k)] || cval[at(k)] == cull_m) {
                ok = false;
            }
        }
        if matches!(op, OP_LOAD | OP_COMPOSITE_CONSTRUCT | OP_COPY_OBJECT) && n >= 3 && at(1) < bound && blk[at(1)] != 0 {
            ok = false;
        }
        if matches!(op, OP_STORE | OP_COPY_MEMORY) && n >= 3 && ((at(1) < bound && bvar[at(1)] != 0) || (at(2) < bound && bvar[at(2)] != 0)) {
            ok = false;
        }
    }
    if !ok {
        return None;
    }
    // the struct loses its last word; the member's name and decorations go
    let mut o = w[..5].to_vec();
    for &(i, op, n) in &v {
        if (op == OP_MEMBER_DECORATE || op == OP_MEMBER_NAME) && n >= 3 && w[i + 1] == st && w[i + 2] == cull_m {
            continue;
        }
        if i == si {
            o.push((((n - 1) as u32) << 16) | op);
            o.extend_from_slice(&w[i + 1..i + n - 1]);
            continue;
        }
        o.extend_from_slice(&w[i..i + n]);
    }
    Some(o)
}

/// Whether gl_PerVertex's clip and cull arrays together pass `limit` (what `trim_cull` could not fix is logged).
pub fn needs_trim(w: &[u32], limit: u32) -> bool {
    let Some(v) = walk(w) else { return false };
    let mut lens = 0u32;
    let mut arr: std::collections::HashMap<u32, u32> = Default::default();
    let mut consts: std::collections::HashMap<u32, u32> = Default::default();
    let mut members: Vec<(u32, u32)> = vec![];
    let mut structs: std::collections::HashMap<u32, Vec<u32>> = Default::default();
    for &(i, op, n) in &v {
        match op {
            OP_CONSTANT if n >= 4 => {
                consts.insert(w[i + 2], w[i + 3]);
            }
            OP_TYPE_ARRAY if n >= 4 => {
                arr.insert(w[i + 1], w[i + 3]);
            }
            OP_TYPE_STRUCT => {
                structs.insert(w[i + 1], w[i + 2..i + n].to_vec());
            }
            OP_MEMBER_DECORATE if n >= 5 && w[i + 3] == DEC_BUILTIN && (w[i + 4] == BI_CLIP || w[i + 4] == BI_CULL) => {
                members.push((w[i + 1], w[i + 2]));
            }
            _ => {}
        }
    }
    for (s, m) in members {
        if let Some(t) = structs.get(&s).and_then(|ms| ms.get(m as usize)) {
            lens += arr.get(t).and_then(|c| consts.get(c)).copied().unwrap_or(0);
        }
    }
    lens > limit
}

/// A user varying: its location, first component, scalar kind (0 float, 1 signed, 2 unsigned) and component count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Varying {
    pub location: u32,
    pub component: u32,
    pub kind: u8,
    pub components: u32,
}

/// The non-builtin Input (`output` false) or Output variables of a module with a Location, each a scalar or vector
/// (others, and a second blend source, are left out).
pub fn io_locations(w: &[u32], output: bool) -> Vec<Varying> {
    let Some(v) = walk(w) else { return vec![] };
    let sc = if output { SC_OUTPUT } else { SC_INPUT };
    let mut loc: std::collections::HashMap<u32, u32> = Default::default();
    let mut comp: std::collections::HashMap<u32, u32> = Default::default();
    let mut builtin = std::collections::HashSet::new();
    let mut index1 = std::collections::HashSet::new();
    let mut types: std::collections::HashMap<u32, (u32, usize)> = Default::default();
    let mut ptrs: std::collections::HashMap<u32, (u32, u32)> = Default::default();
    let mut vars = vec![];
    for &(i, op, n) in &v {
        match op {
            OP_DECORATE if n >= 3 => match w[i + 2] {
                DEC_LOCATION if n >= 4 => {
                    loc.insert(w[i + 1], w[i + 3]);
                }
                DEC_COMPONENT if n >= 4 => {
                    comp.insert(w[i + 1], w[i + 3]);
                }
                DEC_INDEX if n >= 4 && w[i + 3] != 0 => {
                    index1.insert(w[i + 1]);
                }
                DEC_BUILTIN => {
                    builtin.insert(w[i + 1]);
                }
                _ => {}
            },
            OP_TYPE_INT | OP_TYPE_FLOAT | OP_TYPE_VECTOR => {
                types.insert(w[i + 1], (op, i));
            }
            OP_TYPE_POINTER if n >= 4 => {
                ptrs.insert(w[i + 1], (w[i + 2], w[i + 3]));
            }
            OP_VARIABLE if n >= 4 && w[i + 3] == sc => vars.push((w[i + 2], w[i + 1])),
            _ => {}
        }
    }
    let mut out = vec![];
    for (id, pty) in vars {
        if builtin.contains(&id) || index1.contains(&id) {
            continue;
        }
        let (Some(&l), Some(&(_, t))) = (loc.get(&id), ptrs.get(&pty)) else { continue };
        let (mut t, mut nc) = (t, 1u32);
        if let Some(&(op, i)) = types.get(&t) {
            if op == OP_TYPE_VECTOR {
                nc = w[i + 3];
                t = w[i + 2];
            }
        }
        let kind = match types.get(&t) {
            Some(&(OP_TYPE_FLOAT, i)) if w[i + 2] == 32 => 0,
            Some(&(OP_TYPE_INT, i)) if w[i + 2] == 32 => {
                if w[i + 3] == 1 {
                    1
                } else {
                    2
                }
            }
            _ => continue,
        };
        out.push(Varying { location: l, component: comp.get(&id).copied().unwrap_or(0), kind, components: nc });
    }
    out
}

/// The varyings `inputs` (a pixel shader's) reads that `outputs` (its vertex shader's) does not declare at their
/// location (a location declared with other components counts as declared).
pub fn missing(outputs: &[Varying], inputs: &[Varying]) -> Vec<Varying> {
    let mut m: Vec<Varying> = inputs.iter().filter(|i| !outputs.iter().any(|o| o.location == i.location)).copied().collect();
    m.sort_by_key(|v| (v.location, v.component));
    m.dedup_by_key(|v| v.location);
    m
}

/// `w` (a vertex, tessellation or geometry shader) with an Output variable for each of `add`, initialized to
/// (0, 0, 0, 1) cut to its components, decorated with its location and component, and in the entry point's
/// interface. None when `w` is not well-formed or `add` is empty.
pub fn add_outputs(w: &[u32], add: &[Varying]) -> Option<Vec<u32>> {
    let v = walk(w)?;
    if add.is_empty() {
        return None;
    }
    let mut bound = w[3];
    let mut fresh = || {
        let id = bound;
        bound += 1;
        id
    };
    // existing types and constants to reuse (SPIR-V does not allow a scalar or vector type twice)
    let find = |op: u32, ops: &[u32]| -> Option<u32> {
        v.iter().find(|&&(i, o, n)| o == op && n == ops.len() + 2 && &w[i + 2..i + n] == ops).map(|&(i, _, _)| w[i + 1])
    };
    let mut new_types: Vec<Vec<u32>> = vec![];
    let mut made: Vec<(u32, Vec<u32>, u32)> = vec![];
    let mut ty = |op: u32, ops: &[u32], new_types: &mut Vec<Vec<u32>>, fresh: &mut dyn FnMut() -> u32| -> u32 {
        if let Some(id) = find(op, ops) {
            return id;
        }
        if let Some(m) = made.iter().find(|m| m.0 == op && m.1 == ops) {
            return m.2;
        }
        let id = fresh();
        let mut o = vec![id];
        o.extend_from_slice(ops);
        new_types.push(ins(op, &o));
        made.push((op, ops.to_vec(), id));
        id
    };
    let mut consts: Vec<(u32, u32, u32)> = vec![];
    let find_const = |ty: u32, val: u32| -> Option<u32> {
        v.iter().find(|&&(i, o, n)| o == OP_CONSTANT && n == 4 && w[i + 1] == ty && w[i + 3] == val).map(|&(i, _, _)| w[i + 2])
    };
    let mut globals: Vec<Vec<u32>> = vec![];
    let mut decos: Vec<Vec<u32>> = vec![];
    let mut iface: Vec<u32> = vec![];
    for a in add {
        let scalar = match a.kind {
            0 => ty(OP_TYPE_FLOAT, &[32], &mut new_types, &mut fresh),
            1 => ty(OP_TYPE_INT, &[32, 1], &mut new_types, &mut fresh),
            _ => ty(OP_TYPE_INT, &[32, 0], &mut new_types, &mut fresh),
        };
        let nc = a.components.clamp(1, 4);
        let vty = if nc > 1 { ty(OP_TYPE_VECTOR, &[scalar, nc], &mut new_types, &mut fresh) } else { scalar };
        let ptr = ty(OP_TYPE_POINTER, &[SC_OUTPUT, vty], &mut new_types, &mut fresh);
        // (0, 0, 0, 1) from the variable's first component on
        let mut comps = vec![];
        for c in 0..nc {
            let one = a.component + c == 3;
            let bits = match (a.kind, one) {
                (0, true) => 1.0f32.to_bits(),
                (_, true) => 1,
                _ => 0,
            };
            let id = match find_const(scalar, bits).or_else(|| consts.iter().find(|x| x.0 == scalar && x.1 == bits).map(|x| x.2)) {
                Some(id) => id,
                None => {
                    let id = fresh();
                    globals.push(ins(OP_CONSTANT, &[scalar, id, bits]));
                    consts.push((scalar, bits, id));
                    id
                }
            };
            comps.push(id);
        }
        let init = if nc > 1 {
            let id = fresh();
            let mut o = vec![vty, id];
            o.extend_from_slice(&comps);
            globals.push(ins(OP_CONSTANT_COMPOSITE, &o));
            id
        } else {
            comps[0]
        };
        let var = fresh();
        globals.push(ins(OP_VARIABLE, &[ptr, var, SC_OUTPUT, init]));
        decos.push(ins(OP_DECORATE, &[var, DEC_LOCATION, a.location]));
        if a.component != 0 {
            decos.push(ins(OP_DECORATE, &[var, DEC_COMPONENT, a.component]));
        }
        iface.push(var);
    }
    let first_fn = v.iter().find(|x| x.1 == OP_FUNCTION).map(|x| x.0).unwrap_or(w.len());
    let last_deco = v.iter().filter(|x| matches!(x.1, OP_DECORATE | OP_MEMBER_DECORATE | 73 | 74 | 75 | 332 | 5632 | 5633)).map(|x| x.0 + x.2).max();
    let mut o = w[..5].to_vec();
    o[3] = bound;
    let mut placed_decos = false;
    for &(i, op, n) in &v {
        if i == first_fn {
            for t in &new_types {
                o.extend_from_slice(t);
            }
            for g in &globals {
                o.extend_from_slice(g);
            }
        }
        if op == OP_ENTRY_POINT {
            let mut e = w[i + 1..i + n].to_vec();
            e.extend_from_slice(&iface);
            o.extend(ins(OP_ENTRY_POINT, &e));
        } else {
            o.extend_from_slice(&w[i..i + n]);
        }
        if Some(i + n) == last_deco && !placed_decos {
            for d in &decos {
                o.extend_from_slice(d);
            }
            placed_decos = true;
        }
    }
    if first_fn == w.len() {
        return None;
    }
    if !placed_decos {
        return None;
    }
    Some(o)
}

/// A pixel shader writing integer targets (Turbo `vkspv_int_outputs`, 3d77358): the translator's float outputs are
/// only register containers, and an integer attachment takes their bits. `kinds[l]`: 1 the target at location `l` is
/// unsigned, 2 signed, 0 unchanged. The outputs at those locations become integer typed; stores bitcast what the code
/// writes, loads bitcast back. Integer types the module declares after the first retyped output are moved before it.
/// None when it cannot be done (an output absent or shared, an initializer, a copy into an output).
pub fn int_outputs(w: &[u32], kinds: &[u8; 8]) -> Option<Vec<u32>> {
    let v = walk(w)?;
    let bound0 = w[3] as usize;
    let mut bound = w[3];
    let mut def = vec![0usize; bound0];
    let (mut f32t, mut iscal) = (0u32, [0u32; 2]);
    for &(i, op, n) in &v {
        if (19..=OP_TYPE_POINTER).contains(&op) && n >= 2 && (w[i + 1] as usize) < bound0 {
            def[w[i + 1] as usize] = i;
        }
        if matches!(op, OP_VARIABLE | OP_ACCESS_CHAIN | OP_IN_BOUNDS_ACCESS_CHAIN) && n >= 4 && (w[i + 2] as usize) < bound0 {
            def[w[i + 2] as usize] = i;
        }
        if op == OP_TYPE_INT && n == 4 && w[i + 2] == 32 && w[i + 3] < 2 {
            iscal[w[i + 3] as usize] = w[i + 1];
        }
        if op == OP_TYPE_FLOAT && n == 3 && w[i + 2] == 32 {
            f32t = w[i + 1];
        }
    }
    let mut ivec = [[0u32; 5]; 2];
    let mut ivp = [[0u32; 5]; 2];
    let mut isp = [0u32; 2];
    for &(i, op, n) in &v {
        if op == OP_TYPE_VECTOR && n == 4 {
            for k in 0..2 {
                if iscal[k] != 0 && w[i + 2] == iscal[k] && (2..=4).contains(&w[i + 3]) {
                    ivec[k][w[i + 3] as usize] = w[i + 1];
                }
            }
        }
    }
    for &(i, op, n) in &v {
        if op == OP_TYPE_POINTER && n == 4 && w[i + 2] == SC_OUTPUT {
            for k in 0..2 {
                if iscal[k] != 0 && w[i + 3] == iscal[k] {
                    isp[k] = w[i + 1];
                }
                for c in 2..=4 {
                    if ivec[k][c] != 0 && w[i + 3] == ivec[k][c] {
                        ivp[k][c] = w[i + 1];
                    }
                }
            }
        }
    }
    // the outputs by location (-1 absent, -2 shared; builtins and second blend sources left out)
    let mut at = [-1i64; 8];
    {
        let mut out = std::collections::HashSet::new();
        let mut bi = std::collections::HashSet::new();
        for &(i, op, n) in &v {
            if op == OP_VARIABLE && n >= 4 && w[i + 3] == SC_OUTPUT {
                out.insert(w[i + 2]);
            }
            if op == OP_DECORATE && n >= 3 && (w[i + 2] == DEC_BUILTIN || (w[i + 2] == DEC_INDEX && n >= 4 && w[i + 3] == 1)) {
                bi.insert(w[i + 1]);
            }
        }
        for &(i, op, n) in &v {
            if op == OP_DECORATE && n >= 4 && w[i + 2] == DEC_LOCATION && out.contains(&w[i + 1]) && !bi.contains(&w[i + 1]) && w[i + 3] < 8 {
                let l = w[i + 3] as usize;
                at[l] = if at[l] == -1 { w[i + 1] as i64 } else { -2 };
            }
        }
    }
    let mut var_of = [0u32; 8];
    let mut k_of = [0usize; 8];
    let mut nc_of = [0u32; 8];
    let mut any = false;
    for l in 0..8 {
        if kinds[l] == 0 {
            continue;
        }
        if kinds[l] > 2 || at[l] < 0 {
            return None;
        }
        let vi = def[at[l] as usize];
        let pt = if vi != 0 { w[vi + 1] as usize } else { 0 };
        let pti = if pt < bound0 { def[pt] } else { 0 };
        if pti == 0 || w[pti] & 0xffff != OP_TYPE_POINTER || w[pti + 2] != SC_OUTPUT {
            return None;
        }
        let vt = w[pti + 3] as usize;
        let vti = if vt < bound0 { def[vt] } else { 0 };
        nc_of[l] = if vti != 0 && w[vti] & 0xffff == OP_TYPE_FLOAT && w[vti + 2] == 32 {
            1
        } else if vti != 0 && w[vti] & 0xffff == OP_TYPE_VECTOR && f32t != 0 && w[vti + 2] == f32t && (2..=4).contains(&w[vti + 3]) {
            w[vti + 3]
        } else {
            return None;
        };
        var_of[l] = at[l] as u32;
        k_of[l] = kinds[l] as usize - 1;
        any = true;
    }
    if !any {
        return None;
    }
    let first_var = (0..8).filter(|&l| var_of[l] != 0).map(|l| def[var_of[l] as usize]).min()?;
    let mut hoist: Vec<usize> = vec![];
    for k in 0..2 {
        if iscal[k] != 0 && def[iscal[k] as usize] > first_var {
            hoist.push(def[iscal[k] as usize]);
        }
    }
    for k in 0..2 {
        for c in 2..=4 {
            if ivec[k][c] != 0 && def[ivec[k][c] as usize] > first_var {
                hoist.push(def[ivec[k][c] as usize]);
            }
        }
    }
    for k in 0..2 {
        if isp[k] != 0 && def[isp[k] as usize] > first_var {
            hoist.push(def[isp[k] as usize]);
        }
    }
    for k in 0..2 {
        for c in 2..=4 {
            if ivp[k][c] != 0 && def[ivp[k][c] as usize] > first_var {
                hoist.push(def[ivp[k][c] as usize]);
            }
        }
    }
    let mut types: Vec<u32> = vec![];
    for l in 0..8 {
        if var_of[l] == 0 {
            continue;
        }
        let (k, nc) = (k_of[l], nc_of[l] as usize);
        if iscal[k] == 0 {
            iscal[k] = bound;
            bound += 1;
            types.extend(ins(OP_TYPE_INT, &[iscal[k], 32, k as u32]));
        }
        if isp[k] == 0 {
            isp[k] = bound;
            bound += 1;
            types.extend(ins(OP_TYPE_POINTER, &[isp[k], SC_OUTPUT, iscal[k]]));
        }
        if nc > 1 {
            if ivec[k][nc] == 0 {
                ivec[k][nc] = bound;
                bound += 1;
                types.extend(ins(OP_TYPE_VECTOR, &[ivec[k][nc], iscal[k], nc as u32]));
            }
            if ivp[k][nc] == 0 {
                ivp[k][nc] = bound;
                bound += 1;
                types.extend(ins(OP_TYPE_POINTER, &[ivp[k][nc], SC_OUTPUT, ivec[k][nc]]));
            }
        }
    }
    // stores keep the bits; loads cast back, so reads of an output keep their type
    let mut repl: std::collections::HashMap<u32, usize> = Default::default();
    let mut o = w[..5].to_vec();
    for &(i, op, n) in &v {
        if i == first_var {
            for &h in &hoist {
                o.extend_from_slice(&w[h..h + (w[h] >> 16) as usize]);
            }
            o.extend_from_slice(&types);
        }
        if hoist.contains(&i) {
            continue;
        }
        let mut x = w[i..i + n].to_vec();
        if op == OP_VARIABLE && n >= 4 {
            if let Some(l) = (0..8).find(|&l| var_of[l] != 0 && x[2] == var_of[l]) {
                if n != 4 {
                    return None; // an initializer would need retyping too
                }
                x[1] = if nc_of[l] > 1 { ivp[k_of[l]][nc_of[l] as usize] } else { isp[k_of[l]] };
                repl.insert(x[2], k_of[l]);
            }
            o.extend(x);
        } else if (op == OP_ACCESS_CHAIN || op == OP_IN_BOUNDS_ACCESS_CHAIN) && n >= 5 && repl.contains_key(&x[3]) {
            if n != 5 {
                return None;
            }
            let k = repl[&x[3]];
            x[1] = isp[k]; // one index into a vector: the scalar
            repl.insert(x[2], k);
            o.extend(x);
        } else if op == OP_LOAD && n >= 4 && repl.contains_key(&x[3]) {
            let k = repl[&x[3]];
            let ti = def.get(x[1] as usize).copied().unwrap_or(0);
            let nc = if ti != 0 && w[ti] & 0xffff == OP_TYPE_VECTOR { w[ti + 3] as usize } else { 1 };
            let (old_type, old_id) = (x[1], x[2]);
            x[1] = if nc > 1 { ivec[k][nc] } else { iscal[k] };
            x[2] = bound;
            bound += 1;
            let nid = x[2];
            o.extend(x);
            o.extend(ins(OP_BITCAST, &[old_type, old_id, nid]));
        } else if (op == OP_STORE || op == OP_COPY_MEMORY) && n >= 3 && repl.contains_key(&x[1]) {
            if op == OP_COPY_MEMORY || n != 3 {
                return None; // a copy keeps the source's float type
            }
            let k = repl[&x[1]];
            let pt = w[def[x[1] as usize] + 1] as usize;
            let mut nc = 1usize;
            if pt < bound0 && def[pt] != 0 && w[def[pt]] & 0xffff == OP_TYPE_POINTER {
                let vt = w[def[pt] + 3] as usize;
                if vt < bound0 && def[vt] != 0 && w[def[vt]] & 0xffff == OP_TYPE_VECTOR {
                    nc = w[def[vt] + 3] as usize;
                }
            }
            let want = if nc > 1 { ivec[k][nc] } else { iscal[k] };
            let nid = bound;
            bound += 1;
            o.extend(ins(OP_BITCAST, &[want, nid, x[2]]));
            o.extend(ins(OP_STORE, &[x[1], nid]));
        } else {
            o.extend(x);
        }
    }
    o[3] = bound;
    Some(o)
}

/// The non-builtin Output variables at locations 0..8 (-1 absent, -2 more than one; a second blend source, Index 1,
/// left out).
fn outputs_at(w: &[u32], v: &[(usize, u32, usize)]) -> [i64; 8] {
    let mut at = [-1i64; 8];
    let mut out = std::collections::HashSet::new();
    let mut bi = std::collections::HashSet::new();
    for &(i, op, n) in v {
        if op == OP_VARIABLE && n >= 4 && w[i + 3] == SC_OUTPUT {
            out.insert(w[i + 2]);
        }
        if op == OP_DECORATE && n >= 3 && (w[i + 2] == DEC_BUILTIN || (w[i + 2] == DEC_INDEX && n >= 4 && w[i + 3] == 1)) {
            bi.insert(w[i + 1]);
        }
    }
    for &(i, op, n) in v {
        if op == OP_DECORATE && n >= 4 && w[i + 2] == DEC_LOCATION && out.contains(&w[i + 1]) && !bi.contains(&w[i + 1]) && w[i + 3] < 8 {
            let l = w[i + 3] as usize;
            at[l] = if at[l] == -1 { w[i + 1] as i64 } else { -2 };
        }
    }
    at
}

/// Dual-source blending (Turbo `vkspv_dual_source`, nxnative's fs_dual): output 1 becomes Location 0 Index 1, the
/// second source of target 0. Only for a pixel shader writing exactly outputs 0 and 1.
pub fn dual_source(w: &[u32]) -> Option<Vec<u32>> {
    let v = walk(w)?;
    let at = outputs_at(w, &v);
    if at[0] < 0 || at[1] < 0 || at[2..].iter().any(|&x| x >= 0 || x == -2) {
        return None;
    }
    let mut o = w[..5].to_vec();
    for &(i, op, n) in &v {
        if op == OP_DECORATE && n == 4 && w[i + 1] == at[1] as u32 && w[i + 2] == DEC_LOCATION {
            o.extend(ins(OP_DECORATE, &[w[i + 1], DEC_LOCATION, 0]));
            o.extend(ins(OP_DECORATE, &[w[i + 1], DEC_INDEX, 1]));
        } else {
            o.extend_from_slice(&w[i..i + n]);
        }
    }
    Some(o)
}

/// Push constant bytes the alpha test reads: the function (u32, NVN's NEVER = 1 .. ALWAYS = 8) and the reference
/// (f32).
pub const ALPHA_PUSH_OFFSET: u32 = 16;

/// The alpha test (Turbo `vkspv_alpha_test`, nxnative's dmsh_alpha): before each return of the entry point, colour 0's
/// alpha is compared with the reference by NVN's alpha function, both push constants at `ALPHA_PUSH_OFFSET`, and the
/// fragment is discarded when the comparison fails (function − 1 is a mask of less 1, equal 2, greater 4). A module
/// with a push constant block gets the two members appended to it; one without gets a block of its own. Needs colour
/// 0 as a float vec4 output.
pub fn alpha_test(w: &[u32]) -> Option<Vec<u32>> {
    const OP_TYPE_BOOL: u32 = 20;
    const OP_FUNCTION_END: u32 = 56;
    const OP_ISUB: u32 = 130;
    const OP_LOGICAL_OR: u32 = 166;
    const OP_LOGICAL_AND: u32 = 167;
    const OP_INOT_EQUAL: u32 = 171;
    const OP_FORD_EQUAL: u32 = 180;
    const OP_FORD_LESS: u32 = 184;
    const OP_FORD_GREATER: u32 = 186;
    const OP_BITWISE_AND: u32 = 199;
    const OP_SELECTION_MERGE: u32 = 247;
    const OP_LABEL: u32 = 248;
    const OP_BRANCH_COND: u32 = 250;
    const OP_KILL: u32 = 252;
    const OP_TERMINATE_INVOCATION: u32 = 4416;
    const OP_RETURN: u32 = 253;
    const DEC_BLOCK: u32 = 2;
    const DEC_OFFSET: u32 = 35;
    const SC_PUSH: u32 = 9;
    let v = walk(w)?;
    let at = outputs_at(w, &v);
    if at[0] < 0 {
        return None;
    }
    let bound0 = w[3] as usize;
    let mut def = vec![0usize; bound0];
    let (mut entry, mut f32t, mut u32t, mut tbool, mut pof) = (0u32, 0u32, 0u32, 0u32, 0u32);
    let (mut first_type, mut first_fn) = (0usize, 0usize);
    let mut push_var: Option<(u32, u32)> = None; // (variable, its pointer type)
    for &(i, op, n) in &v {
        if (19..=OP_TYPE_POINTER).contains(&op) && n >= 2 && (w[i + 1] as usize) < bound0 {
            def[w[i + 1] as usize] = i;
            if first_type == 0 {
                first_type = i;
            }
        }
        if op == OP_VARIABLE && n >= 4 && (w[i + 2] as usize) < bound0 {
            def[w[i + 2] as usize] = i;
            if w[i + 3] == SC_PUSH {
                if push_var.is_some() {
                    return None;
                }
                push_var = Some((w[i + 2], w[i + 1]));
            }
        }
        if op == OP_ENTRY_POINT && n >= 3 && entry == 0 {
            entry = w[i + 2];
        }
        if op == OP_TYPE_FLOAT && n == 3 && w[i + 2] == 32 {
            f32t = w[i + 1];
        }
        if op == OP_TYPE_INT && n == 4 && w[i + 2] == 32 && w[i + 3] == 0 {
            u32t = w[i + 1];
        }
        if op == OP_TYPE_BOOL && n == 2 {
            tbool = w[i + 1];
        }
        if op == OP_FUNCTION && first_fn == 0 {
            first_fn = i;
        }
    }
    // colour 0: an Output pointer to 4 floats
    let vi = def[at[0] as usize];
    let pt = if vi != 0 { w[vi + 1] as usize } else { 0 };
    let pti = if pt < bound0 { def[pt] } else { 0 };
    let vt = if pti != 0 && w[pti] & 0xffff == OP_TYPE_POINTER { w[pti + 3] as usize } else { 0 };
    let vti = if vt < bound0 { def[vt] } else { 0 };
    if entry == 0 || f32t == 0 || first_type == 0 || first_fn == 0 || vti == 0 || w[vti] & 0xffff != OP_TYPE_VECTOR || w[vti + 2] != f32t || w[vti + 3] != 4 {
        return None;
    }
    for &(i, op, n) in &v {
        if op == OP_TYPE_POINTER && n == 4 && w[i + 2] == SC_OUTPUT && w[i + 3] == f32t {
            pof = w[i + 1];
        }
    }
    let mut bound = w[3];
    let mut id = || {
        let x = bound;
        bound += 1;
        x
    };
    let mut ty: Vec<u32> = vec![];
    let mut dec: Vec<u32> = vec![];
    if u32t == 0 {
        if push_var.is_some() {
            return None; // the module's push block would name a type declared after it
        }
        u32t = id();
        ty.extend(ins(OP_TYPE_INT, &[u32t, 32, 0]));
    }
    if tbool == 0 {
        tbool = id();
        ty.extend(ins(OP_TYPE_BOOL, &[tbool]));
    }
    if pof == 0 {
        pof = id();
        ty.extend(ins(OP_TYPE_POINTER, &[pof, SC_OUTPUT, f32t]));
    }
    // the push block: the module's own with two members appended, or one of its own
    let (var, m_func, m_ref, old_struct) = match push_var {
        Some((pv, pvt)) => {
            let pti = def.get(pvt as usize).copied().unwrap_or(0);
            if pti == 0 || w[pti] & 0xffff != OP_TYPE_POINTER {
                return None;
            }
            let st = w[pti + 3];
            let sti = def.get(st as usize).copied().unwrap_or(0);
            if sti == 0 || w[sti] & 0xffff != OP_TYPE_STRUCT {
                return None;
            }
            // the struct must be used by this block only, and its members must end before the new ones
            let uses = v.iter().filter(|&&(i, op, n)| op == OP_TYPE_POINTER && n == 4 && w[i + 3] == st).count();
            if uses != 1 {
                return None;
            }
            let nm = (w[sti] >> 16) - 2;
            for &(i, op, n) in &v {
                if op == OP_MEMBER_DECORATE && n >= 5 && w[i + 1] == st && w[i + 3] == DEC_OFFSET && w[i + 4] >= ALPHA_PUSH_OFFSET {
                    return None;
                }
            }
            (pv, nm, nm + 1, Some((sti, st)))
        }
        None => {
            let st = id();
            let pst = id();
            let pv = id();
            ty.extend(ins(OP_TYPE_STRUCT, &[st, u32t, f32t]));
            ty.extend(ins(OP_TYPE_POINTER, &[pst, SC_PUSH, st]));
            ty.extend(ins(OP_VARIABLE, &[pst, pv, SC_PUSH]));
            dec.extend(ins(OP_DECORATE, &[st, DEC_BLOCK]));
            dec.extend(ins(OP_MEMBER_DECORATE, &[st, 0, DEC_OFFSET, ALPHA_PUSH_OFFSET]));
            dec.extend(ins(OP_MEMBER_DECORATE, &[st, 1, DEC_OFFSET, ALPHA_PUSH_OFFSET + 4]));
            (pv, 0, 1, None)
        }
    };
    if let Some((_, st)) = old_struct {
        dec.extend(ins(OP_MEMBER_DECORATE, &[st, m_func, DEC_OFFSET, ALPHA_PUSH_OFFSET]));
        dec.extend(ins(OP_MEMBER_DECORATE, &[st, m_ref, DEC_OFFSET, ALPHA_PUSH_OFFSET + 4]));
    }
    let pu = id();
    let pfl = id();
    ty.extend(ins(OP_TYPE_POINTER, &[pu, SC_PUSH, u32t]));
    ty.extend(ins(OP_TYPE_POINTER, &[pfl, SC_PUSH, f32t]));
    let mut c = [0u32; 5];
    for (k, cc) in c.iter_mut().enumerate() {
        *cc = id();
        ty.extend(ins(OP_CONSTANT, &[u32t, *cc, k as u32]));
    }
    let (cm_func, cm_ref) = if old_struct.is_some() {
        let a = id();
        let b = id();
        ty.extend(ins(OP_CONSTANT, &[u32t, a, m_func]));
        ty.extend(ins(OP_CONSTANT, &[u32t, b, m_ref]));
        (a, b)
    } else {
        (c[0], c[1])
    };
    let kill = if w[1] >= 0x0001_0600 { OP_TERMINATE_INVOCATION } else { OP_KILL };
    let mut o = w[..5].to_vec();
    let mut in_fn = 0u32;
    for &(i, op, n) in &v {
        if i == first_type {
            o.extend_from_slice(&dec);
        }
        if i == first_fn {
            o.extend_from_slice(&ty);
        }
        if op == OP_FUNCTION {
            in_fn = w[i + 2];
        }
        if op == OP_RETURN && in_fn == entry {
            let (pf, f, pr, r, pa, a, m, lt, eq, gt) = (id(), id(), id(), id(), id(), id(), id(), id(), id(), id());
            let (b0, h0, p0, b1, h1, p1, b2, h2, p2, q, pass, ok_l, kill_l) = (id(), id(), id(), id(), id(), id(), id(), id(), id(), id(), id(), id(), id());
            o.extend(ins(OP_ACCESS_CHAIN, &[pu, pf, var, cm_func]));
            o.extend(ins(OP_LOAD, &[u32t, f, pf]));
            o.extend(ins(OP_ACCESS_CHAIN, &[pfl, pr, var, cm_ref]));
            o.extend(ins(OP_LOAD, &[f32t, r, pr]));
            o.extend(ins(OP_ACCESS_CHAIN, &[pof, pa, at[0] as u32, c[3]]));
            o.extend(ins(OP_LOAD, &[f32t, a, pa]));
            o.extend(ins(OP_ISUB, &[u32t, m, f, c[1]]));
            o.extend(ins(OP_FORD_LESS, &[tbool, lt, a, r]));
            o.extend(ins(OP_FORD_EQUAL, &[tbool, eq, a, r]));
            o.extend(ins(OP_FORD_GREATER, &[tbool, gt, a, r]));
            o.extend(ins(OP_BITWISE_AND, &[u32t, b0, m, c[1]]));
            o.extend(ins(OP_INOT_EQUAL, &[tbool, h0, b0, c[0]]));
            o.extend(ins(OP_LOGICAL_AND, &[tbool, p0, h0, lt]));
            o.extend(ins(OP_BITWISE_AND, &[u32t, b1, m, c[2]]));
            o.extend(ins(OP_INOT_EQUAL, &[tbool, h1, b1, c[0]]));
            o.extend(ins(OP_LOGICAL_AND, &[tbool, p1, h1, eq]));
            o.extend(ins(OP_BITWISE_AND, &[u32t, b2, m, c[4]]));
            o.extend(ins(OP_INOT_EQUAL, &[tbool, h2, b2, c[0]]));
            o.extend(ins(OP_LOGICAL_AND, &[tbool, p2, h2, gt]));
            o.extend(ins(OP_LOGICAL_OR, &[tbool, q, p0, p1]));
            o.extend(ins(OP_LOGICAL_OR, &[tbool, pass, q, p2]));
            o.extend(ins(OP_SELECTION_MERGE, &[ok_l, 0]));
            o.extend(ins(OP_BRANCH_COND, &[pass, ok_l, kill_l]));
            o.extend(ins(OP_LABEL, &[kill_l]));
            o.extend(ins(kill, &[]));
            o.extend(ins(OP_LABEL, &[ok_l]));
        }
        if op == OP_FUNCTION_END {
            in_fn = 0;
        }
        if op == OP_ENTRY_POINT && w[i + 2] == entry && w[1] >= 0x0001_0400 && push_var.is_none() {
            // SPIR-V 1.4 on: an entry point lists every global it uses
            let mut e = w[i + 1..i + n].to_vec();
            e.push(var);
            o.extend(ins(OP_ENTRY_POINT, &e));
            continue;
        }
        if let Some((sti, _)) = old_struct {
            if i == sti {
                let mut s = w[i + 1..i + n].to_vec();
                s.push(u32t);
                s.push(f32t);
                o.extend(ins(OP_TYPE_STRUCT, &s));
                continue;
            }
        }
        o.extend_from_slice(&w[i..i + n]);
    }
    o[3] = bound;
    Some(o)
}

/// NVN's compiler aggregates an atomic add over a warp: ballot(true), the highest lane leads and adds the bit count
/// for all. After some lanes have returned that ballot never completes on NVIDIA's RTX 50 cards (Turbo #12, shader
/// d2c50cca6e386abc, driver 617). Where the add's result is never read, a ballot of only the lane's own bit gives the
/// same sum (every lane leads itself and adds 1). Only that shape is rewritten (Turbo `vkspv_ballot_per_lane`): a
/// ballot(true) reaching a FindUMsb / FindSMsb and a bit count, the bit count an OpAtomicIAdd whose result goes nowhere
/// (or only into a variable nothing reads). → (the new words, ballots rewritten); None when nothing changed.
pub fn ballot_per_lane(w: &[u32]) -> Option<(Vec<u32>, u32)> {
    const B_NAME: u32 = 5;
    const B_EXT_INST: u32 = 12;
    const B_CONSTANT_TRUE: u32 = 41;
    const B_SHL: u32 = 196;
    const B_AND: u32 = 199;
    const B_BIT_COUNT: u32 = 205;
    const B_ATOMIC_IADD: u32 = 234;
    const B_BALLOT: u32 = 339;
    const B_COMPOSITE_EXTRACT: u32 = 81;
    const BI_SUBGROUP_LANE: u32 = 41;
    const FIND_SMSB: u32 = 74;
    const FIND_UMSB: u32 = 75;
    let v = walk(w)?;
    let bound0 = w[3] as usize;
    let mut def = vec![0usize; bound0];
    let mut is_true = vec![false; bound0];
    let (mut tu, mut c0, mut c1, mut lane_var, mut first_fn) = (0u32, 0u32, 0u32, 0u32, 0usize);
    let mut ballots: Vec<(u32, usize)> = vec![];
    for &(i, op, n) in &v {
        let lt = |x: u32| (x as usize) < bound0;
        if matches!(op, OP_TYPE_INT | OP_TYPE_VECTOR | OP_TYPE_POINTER) && n >= 2 && lt(w[i + 1]) {
            def[w[i + 1] as usize] = i;
        }
        if op == OP_VARIABLE && n >= 4 && lt(w[i + 2]) {
            def[w[i + 2] as usize] = i;
        }
        if op == OP_TYPE_INT && n == 4 && w[i + 2] == 32 && w[i + 3] == 0 && tu == 0 {
            tu = w[i + 1];
        }
        if op == B_CONSTANT_TRUE && n == 3 && lt(w[i + 2]) {
            is_true[w[i + 2] as usize] = true;
        }
        if op == OP_CONSTANT && n == 4 && tu != 0 && w[i + 1] == tu {
            if w[i + 3] == 0 && c0 == 0 {
                c0 = w[i + 2];
            }
            if w[i + 3] == 1 && c1 == 0 {
                c1 = w[i + 2];
            }
        }
        if op == OP_DECORATE && n == 4 && w[i + 2] == DEC_BUILTIN && w[i + 3] == BI_SUBGROUP_LANE {
            lane_var = w[i + 1];
        }
        if op == OP_FUNCTION && first_fn == 0 {
            first_fn = i;
        }
        if op == B_BALLOT && n == 5 && lt(w[i + 4]) && is_true[w[i + 4] as usize] && ballots.len() < 32 {
            ballots.push((w[i + 2], i));
        }
    }
    let lane_ok = lane_var != 0 && (lane_var as usize) < bound0 && def[lane_var as usize] != 0 && {
        let pt = w[def[lane_var as usize] + 1] as usize;
        pt < bound0 && def[pt] != 0 && w[def[pt]] & 0xffff == OP_TYPE_POINTER && w[def[pt] + 3] == tu
    };
    if ballots.is_empty() || tu == 0 || !lane_ok || first_fn == 0 {
        return None;
    }
    // what each ballot's value reaches (bit k for ballot k): D its bits, C a bit count of them
    let mut d = vec![0u32; bound0];
    let mut c = vec![0u32; bound0];
    for (k, &(r, _)) in ballots.iter().enumerate() {
        d[r as usize] |= 1 << k;
    }
    let (mut msb, mut atom, mut atom_used) = (0u32, 0u32, 0u32);
    let ok_id = |x: u32| (x as usize) < bound0;
    for _ in 0..32 {
        let mut changed = false;
        atom = 0;
        let mut flow = |d: &mut Vec<u32>, c: &mut Vec<u32>, dst: u32, dd: u32, cc: u32| {
            let t = dst as usize;
            if (d[t] | dd) != d[t] || (c[t] | cc) != c[t] {
                d[t] |= dd;
                c[t] |= cc;
                changed = true;
            }
        };
        for &(i, op, n) in &v {
            if op == OP_STORE && n >= 3 && ok_id(w[i + 1]) && ok_id(w[i + 2]) {
                let (dd, cc) = (d[w[i + 2] as usize], c[w[i + 2] as usize]);
                flow(&mut d, &mut c, w[i + 1], dd, cc);
            } else if matches!(op, OP_LOAD | OP_ACCESS_CHAIN | OP_IN_BOUNDS_ACCESS_CHAIN | B_COMPOSITE_EXTRACT | OP_COPY_OBJECT | OP_BITCAST) && n >= 4 && ok_id(w[i + 2]) && ok_id(w[i + 3]) {
                let (dd, cc) = (d[w[i + 3] as usize], c[w[i + 3] as usize]);
                flow(&mut d, &mut c, w[i + 2], dd, cc);
            } else if op == B_AND && n == 5 && ok_id(w[i + 2]) && ok_id(w[i + 3]) && ok_id(w[i + 4]) {
                let (a, b) = (w[i + 3] as usize, w[i + 4] as usize);
                let (dd, cc) = (d[a] | d[b], c[a] | c[b]);
                flow(&mut d, &mut c, w[i + 2], dd, cc);
            } else if op == B_BIT_COUNT && n == 4 && ok_id(w[i + 2]) && ok_id(w[i + 3]) {
                let s = w[i + 3] as usize;
                let cc = d[s] | c[s];
                flow(&mut d, &mut c, w[i + 2], 0, cc);
            } else if op == B_EXT_INST && n == 6 && (w[i + 4] == FIND_UMSB || w[i + 4] == FIND_SMSB) && ok_id(w[i + 5]) {
                msb |= d[w[i + 5] as usize];
            } else if op == B_ATOMIC_IADD && n == 7 && ok_id(w[i + 6]) && c[w[i + 6] as usize] != 0 {
                let cv = c[w[i + 6] as usize];
                atom |= cv;
                // its result read anywhere (beyond a store into a variable nothing else names) keeps those ballots
                let r = w[i + 2];
                let mut stored = 0u32;
                for &(j, jo, jc) in &v {
                    if atom_used & cv != 0 {
                        break;
                    }
                    if j == i {
                        continue;
                    }
                    if let Some(a) = (1..jc).find(|&a| w[j + a] == r) {
                        if jo == OP_STORE && a == 2 && stored == 0 {
                            stored = w[j + 1];
                        } else {
                            atom_used |= cv;
                        }
                    }
                }
                if stored != 0 {
                    for &(j, jo, jc) in &v {
                        if atom_used & cv != 0 {
                            break;
                        }
                        if jo == B_NAME || (jo == OP_VARIABLE && w[j + 2] == stored) || (jo == OP_STORE && w[j + 1] == stored) {
                            continue;
                        }
                        if (1..jc).any(|a| w[j + a] == stored) {
                            atom_used |= cv;
                        }
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
    let mut ok = msb & atom & !atom_used;
    for (k, &(_, at)) in ballots.iter().enumerate() {
        if ok >> k & 1 != 0 {
            let vt = w[at + 1] as usize;
            if !(vt < bound0 && def[vt] != 0 && w[def[vt]] & 0xffff == OP_TYPE_VECTOR && w[def[vt] + 2] == tu && w[def[vt] + 3] == 4) {
                ok &= !(1 << k);
            }
        }
    }
    if ok == 0 {
        return None;
    }
    let mut bound = w[3];
    let mut o = w[..5].to_vec();
    let mut nrw = 0;
    for &(i, _, n) in &v {
        if i == first_fn {
            if c0 == 0 {
                c0 = bound;
                bound += 1;
                o.extend(ins(OP_CONSTANT, &[tu, c0, 0]));
            }
            if c1 == 0 {
                c1 = bound;
                bound += 1;
                o.extend(ins(OP_CONSTANT, &[tu, c1, 1]));
            }
        }
        if let Some(_k) = ballots.iter().enumerate().find(|(k, b)| b.1 == i && ok >> k & 1 != 0) {
            let (lid, bit) = (bound, bound + 1);
            bound += 2;
            o.extend(ins(OP_LOAD, &[tu, lid, lane_var]));
            o.extend(ins(B_SHL, &[tu, bit, c1, lid]));
            o.extend(ins(OP_COMPOSITE_CONSTRUCT, &[w[i + 1], w[i + 2], bit, c0, c0, c0]));
            nrw += 1;
            continue;
        }
        o.extend_from_slice(&w[i..i + n]);
    }
    o[3] = bound;
    Some((o, nrw))
}

/// A bound on every loop (GPU hangs): a translated loop whose exit depends on data that came out garbage (an unbound
/// bank read as zeros, a count from memory nobody wrote) never ends, the driver resets the GPU after ~2 s and the
/// device is lost. Each function gets a counter of back-edges taken (a Function variable starting at 0); at every
/// loop's back-edge the counter goes up and, once it reaches `cap`, the back-edge goes to the loop's merge block
/// instead (the loop ends as if its condition had failed). → (the new words, loops guarded); None when nothing
/// changed. Loops whose back-edge is not a plain or two-way branch to the header / merge are left as they are.
pub fn loop_guard(w: &[u32], cap: u32) -> Option<(Vec<u32>, u32)> {
    const OP_TYPE_BOOL: u32 = 20;
    const OP_FUNCTION_END: u32 = 56;
    const OP_LABEL: u32 = 248;
    const OP_BRANCH: u32 = 249;
    const OP_BRANCH_COND: u32 = 250;
    const OP_LOOP_MERGE: u32 = 246;
    const OP_IADD: u32 = 128;
    const OP_ULESS_THAN: u32 = 176;
    const OP_LOGICAL_AND: u32 = 167;
    const OP_LOGICAL_OR: u32 = 166;
    const OP_LOGICAL_NOT: u32 = 168;
    const SC_FUNCTION: u32 = 7;
    let v = walk(w)?;
    // loops: header label -> merge label, per function (labels are unique in the module)
    let mut merge_of: std::collections::HashMap<u32, u32> = Default::default();
    let mut cur_label = 0u32;
    for &(i, op, _) in &v {
        if op == OP_LABEL {
            cur_label = w[i + 1];
        }
        if op == OP_LOOP_MERGE {
            merge_of.insert(cur_label, w[i + 1]);
        }
    }
    if merge_of.is_empty() {
        return None;
    }
    // back-edges: a block after its loop's header whose terminator branches to the header
    let mut seen_labels = std::collections::HashSet::new();
    let mut edges: Vec<(usize, u32)> = vec![]; // (terminator index, header)
    for &(i, op, n) in &v {
        match op {
            OP_LABEL => {
                seen_labels.insert(w[i + 1]);
            }
            OP_BRANCH if n == 2 && merge_of.contains_key(&w[i + 1]) && seen_labels.contains(&w[i + 1]) => edges.push((i, w[i + 1])),
            OP_BRANCH_COND if n >= 4 => {
                let (t, f) = (w[i + 2], w[i + 3]);
                for h in [t, f] {
                    if let Some(&m) = merge_of.get(&h) {
                        let other = if h == t { f } else { t };
                        if seen_labels.contains(&h) && other == m {
                            edges.push((i, h));
                        }
                    }
                }
            }
            OP_FUNCTION => seen_labels.clear(),
            _ => {}
        }
    }
    if edges.is_empty() {
        return None;
    }
    let bound0 = w[3];
    let mut bound = bound0;
    let mut id = || {
        let x = bound;
        bound += 1;
        x
    };
    let find = |op: u32, ops: &[u32]| -> Option<u32> { v.iter().find(|&&(i, o, n)| o == op && n == ops.len() + 2 && &w[i + 2..i + n] == ops).map(|&(i, _, _)| w[i + 1]) };
    let mut globals: Vec<u32> = vec![];
    let u32t = match find(OP_TYPE_INT, &[32, 0]) {
        Some(t) => t,
        None => {
            let t = id();
            globals.extend(ins(OP_TYPE_INT, &[t, 32, 0]));
            t
        }
    };
    let boolt = match v.iter().find(|&&(_, o, n)| o == OP_TYPE_BOOL && n == 2) {
        Some(&(i, _, _)) => w[i + 1],
        None => {
            let t = id();
            globals.extend(ins(OP_TYPE_BOOL, &[t]));
            t
        }
    };
    let ptr = match find(OP_TYPE_POINTER, &[SC_FUNCTION, u32t]) {
        Some(t) => t,
        None => {
            let t = id();
            globals.extend(ins(OP_TYPE_POINTER, &[t, SC_FUNCTION, u32t]));
            t
        }
    };
    let (c0, c1, ccap) = (id(), id(), id());
    globals.extend(ins(OP_CONSTANT, &[u32t, c0, 0]));
    globals.extend(ins(OP_CONSTANT, &[u32t, c1, 1]));
    globals.extend(ins(OP_CONSTANT, &[u32t, ccap, cap]));
    let first_fn = v.iter().find(|x| x.1 == OP_FUNCTION).map(|x| x.0)?;
    let edge_at: std::collections::HashMap<usize, u32> = edges.iter().copied().collect();
    let mut o = w[..5].to_vec();
    let mut counter = 0u32; // this function's counter variable
    let mut want_var = false; // the next label is the function's first block
    let mut pending_var = false; // insert the variable once the first block's variables are past
    let mut n_guard = 0u32;
    for &(i, op, n) in &v {
        if i == first_fn {
            o.extend_from_slice(&globals);
        }
        if pending_var && op != OP_VARIABLE {
            o.extend(ins(OP_VARIABLE, &[ptr, counter, SC_FUNCTION, c0]));
            pending_var = false;
        }
        if op == OP_FUNCTION {
            counter = id();
            want_var = true;
        }
        if let Some(&h) = edge_at.get(&i) {
            let m = merge_of[&h];
            let (ld, add, ok) = (id(), id(), id());
            o.extend(ins(OP_LOAD, &[u32t, ld, counter]));
            o.extend(ins(OP_IADD, &[u32t, add, ld, c1]));
            o.extend(ins(OP_STORE, &[counter, add]));
            o.extend(ins(OP_ULESS_THAN, &[boolt, ok, add, ccap]));
            if op == OP_BRANCH {
                o.extend(ins(OP_BRANCH_COND, &[ok, h, m]));
            } else {
                let (c, t) = (w[i + 1], w[i + 2]);
                let nc = id();
                if t == h {
                    // c ? header : merge  ->  (c && ok) ? header : merge
                    o.extend(ins(OP_LOGICAL_AND, &[boolt, nc, c, ok]));
                } else {
                    // c ? merge : header  ->  (c || !ok) ? merge : header
                    let nok = id();
                    o.extend(ins(OP_LOGICAL_NOT, &[boolt, nok, ok]));
                    o.extend(ins(OP_LOGICAL_OR, &[boolt, nc, c, nok]));
                }
                let mut x = w[i..i + n].to_vec();
                x[1] = nc;
                o.extend(x);
            }
            n_guard += 1;
            continue;
        }
        o.extend_from_slice(&w[i..i + n]);
        if op == OP_LABEL && want_var {
            want_var = false;
            pending_var = true;
        }
        if op == OP_FUNCTION_END {
            want_var = false;
        }
    }
    o[3] = bound;
    Some((o, n_guard))
}

/// The scalar kind (0 float, 1 signed, 2 unsigned; `Varying::kind`) of each output location 0..8 of a pixel shader
/// (None where it has none).
pub fn output_kinds(w: &[u32]) -> [Option<u8>; 8] {
    let mut r = [None; 8];
    for o in io_locations(w, true) {
        if o.location < 8 {
            r[o.location as usize] = Some(o.kind);
        }
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny vertex-like module: entry point, a float vec4 output at location 0, gl_PerVertex with ClipDistance[8]
    /// and CullDistance[1] written only through its position member.
    fn module() -> Vec<u32> {
        let mut w = vec![0x0723_0203, 0x0001_0300, 0, 40, 0];
        let push = |w: &mut Vec<u32>, op: u32, ops: &[u32]| w.extend(ins(op, ops));
        push(&mut w, 17, &[1]); // Capability Shader
        push(&mut w, 14, &[0, 1]); // MemoryModel
        push(&mut w, OP_ENTRY_POINT, &[0, 1, 0x6e69616d, 0, 2, 20]); // Vertex %1 "main" %2 %20
        push(&mut w, OP_DECORATE, &[2, DEC_LOCATION, 0]);
        push(&mut w, OP_MEMBER_DECORATE, &[10, 0, DEC_BUILTIN, 0]);
        push(&mut w, OP_MEMBER_DECORATE, &[10, 1, DEC_BUILTIN, BI_CLIP]);
        push(&mut w, OP_MEMBER_DECORATE, &[10, 2, DEC_BUILTIN, BI_CULL]);
        push(&mut w, 19, &[3]); // void
        push(&mut w, 33, &[4, 3]); // fn type
        push(&mut w, OP_TYPE_FLOAT, &[5, 32]);
        push(&mut w, OP_TYPE_VECTOR, &[6, 5, 4]);
        push(&mut w, OP_TYPE_POINTER, &[7, SC_OUTPUT, 6]);
        push(&mut w, OP_VARIABLE, &[7, 2, SC_OUTPUT]);
        push(&mut w, OP_TYPE_INT, &[8, 32, 0]);
        push(&mut w, OP_CONSTANT, &[8, 9, 8]);
        push(&mut w, OP_CONSTANT, &[8, 11, 1]);
        push(&mut w, OP_CONSTANT, &[8, 21, 0]);
        push(&mut w, OP_TYPE_ARRAY, &[12, 5, 9]);
        push(&mut w, OP_TYPE_ARRAY, &[13, 5, 11]);
        push(&mut w, OP_TYPE_STRUCT, &[10, 6, 12, 13]);
        push(&mut w, OP_TYPE_POINTER, &[14, SC_OUTPUT, 10]);
        push(&mut w, OP_VARIABLE, &[14, 20, SC_OUTPUT]);
        push(&mut w, OP_FUNCTION, &[3, 1, 0, 4]);
        push(&mut w, 248, &[30]);
        push(&mut w, OP_ACCESS_CHAIN, &[7, 31, 20, 21]);
        push(&mut w, 253, &[]);
        push(&mut w, 56, &[]);
        w
    }

    #[test]
    fn cull_member_trimmed_when_unused() {
        let w = module();
        assert!(needs_trim(&w, 8));
        let t = trim_cull(&w, 8).expect("trimmed");
        assert!(!needs_trim(&t, 8));
        assert_eq!(t.len(), w.len() - 1 - 5, "struct one word shorter, the cull member decoration gone");
        assert!(trim_cull(&w, 9).is_none(), "within the limit: unchanged");
    }

    #[test]
    fn float_outputs_retyped_to_integers() {
        // a pixel-like module: a vec4 output at location 0, stored once
        let mut w = vec![0x0723_0203, 0x0001_0300, 0, 20, 0];
        let push = |w: &mut Vec<u32>, op: u32, ops: &[u32]| w.extend(ins(op, ops));
        push(&mut w, 17, &[1]);
        push(&mut w, 14, &[0, 1]);
        push(&mut w, OP_ENTRY_POINT, &[4, 1, 0x6e69616d, 0, 2]);
        push(&mut w, OP_DECORATE, &[2, DEC_LOCATION, 0]);
        push(&mut w, 19, &[3]);
        push(&mut w, 33, &[4, 3]);
        push(&mut w, OP_TYPE_FLOAT, &[5, 32]);
        push(&mut w, OP_TYPE_VECTOR, &[6, 5, 4]);
        push(&mut w, OP_TYPE_POINTER, &[7, SC_OUTPUT, 6]);
        push(&mut w, OP_VARIABLE, &[7, 2, SC_OUTPUT]);
        push(&mut w, 46, &[6, 8]); // OpConstantNull vec4
        push(&mut w, OP_FUNCTION, &[3, 1, 0, 4]);
        push(&mut w, 248, &[9]);
        push(&mut w, OP_STORE, &[2, 8]);
        push(&mut w, 253, &[]);
        push(&mut w, 56, &[]);
        assert_eq!(output_kinds(&w)[0], Some(0));
        let o = int_outputs(&w, &[2, 0, 0, 0, 0, 0, 0, 0]).expect("variant");
        assert_eq!(output_kinds(&o)[0], Some(1), "signed");
        let o = int_outputs(&w, &[1, 0, 0, 0, 0, 0, 0, 0]).expect("variant");
        assert_eq!(output_kinds(&o)[0], Some(2), "unsigned");
        assert!(int_outputs(&w, &[0, 2, 0, 0, 0, 0, 0, 0]).is_none(), "no output at location 1");
    }

    #[test]
    fn missing_outputs_are_added() {
        let w = module();
        let outs = io_locations(&w, true);
        assert_eq!(outs, vec![Varying { location: 0, component: 0, kind: 0, components: 4 }]);
        let want = [Varying { location: 0, component: 0, kind: 0, components: 4 }, Varying { location: 3, component: 0, kind: 2, components: 2 }];
        let m = missing(&outs, &want);
        assert_eq!(m.len(), 1);
        let a = add_outputs(&w, &m).expect("added");
        let o2 = io_locations(&a, true);
        assert_eq!(o2.len(), 2);
        assert!(o2.contains(&Varying { location: 3, component: 0, kind: 2, components: 2 }));
        assert!(missing(&o2, &want).is_empty());
    }
}
