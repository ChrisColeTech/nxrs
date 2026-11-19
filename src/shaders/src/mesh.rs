//! NVN2 task and mesh shaders as Vulkan task and mesh stages (`VK_EXT_mesh_shader`).
//!
//! The translator reads a task or mesh shader as a compute kernel whose stage memory lives in its one workgroup array
//! (job.rs `mesh_stage`: the workgroup's number at word 0, the input from 0x10, then the outputs and, for a mesh
//! shader, the primitive count and index bytes). This pass keeps that kernel as an ordinary function and adds the
//! stage's own entry point around it:
//!
//! 1. the stage memory is cleared (every invocation a share of it), the workgroup's number is stored (and, for a mesh
//!    shader launched by a task, the task's payload is copied in, its word 0 being the mesh workgroup's number);
//! 2. the kernel runs;
//! 3. a task shader copies its payload out and launches `min(word 1 of its outputs, 65535)` mesh workgroups; a mesh
//!    shader declares its vertices (all `max_v`) and the primitive count the kernel wrote (byte 3 of the primitive
//!    word, at most `max_p`), and copies the attribute blocks out: vertex v's component k at
//!    `out + (v / 32) * 32 * nv + 32 * k + v % 32` (words), primitives' after all vertex blocks, their system values
//!    (primitive ID, layer, viewport) first. Per-primitive generic attributes go to the primitive's vertices (the
//!    pixel shader declares them per vertex); the index bytes become the primitive index array.
//!
//! The kernel's subgroup lane number becomes the workgroup's invocation index (a console workgroup is one warp; a PC
//! may split it). The module becomes SPIR-V 1.4 (its entry point lists every global variable). The draw's first
//! workgroup is 0: Vulkan has no parameter for it (graphics says so in red when a game asks for another).

use serde_json::Value;

/// Part of a task / mesh output's identity (prebuild.rs): a change here translates those shaders again.
pub const VERSION: &str = "mesh 1";

const OP_EXTENSION: u32 = 10;
const OP_MEMORY_MODEL: u32 = 14;
const OP_ENTRY_POINT: u32 = 15;
const OP_EXECUTION_MODE: u32 = 16;
const OP_CAPABILITY: u32 = 17;
const OP_TYPE_VOID: u32 = 19;
const OP_TYPE_BOOL: u32 = 20;
const OP_TYPE_INT: u32 = 21;
const OP_TYPE_FLOAT: u32 = 22;
const OP_TYPE_VECTOR: u32 = 23;
const OP_TYPE_ARRAY: u32 = 28;
const OP_TYPE_POINTER: u32 = 32;
const OP_TYPE_FUNCTION: u32 = 33;
const OP_CONSTANT: u32 = 43;
const OP_CONSTANT_NULL: u32 = 46;
const OP_FUNCTION: u32 = 54;
const OP_FUNCTION_END: u32 = 56;
const OP_FUNCTION_CALL: u32 = 57;
const OP_VARIABLE: u32 = 59;
const OP_LOAD: u32 = 61;
const OP_STORE: u32 = 62;
const OP_ACCESS_CHAIN: u32 = 65;
const OP_DECORATE: u32 = 71;
const OP_COMPOSITE_CONSTRUCT: u32 = 80;
const OP_COMPOSITE_EXTRACT: u32 = 81;
const OP_BITCAST: u32 = 124;
const OP_IADD: u32 = 128;
const OP_IMUL: u32 = 132;
const OP_UDIV: u32 = 134;
const OP_UMOD: u32 = 137;
const OP_IEQUAL: u32 = 170;
const OP_ULESS_THAN: u32 = 176;
const OP_SELECT: u32 = 169;
const OP_SHIFT_RIGHT_LOGICAL: u32 = 194;
const OP_BITWISE_AND: u32 = 199;
const OP_CONTROL_BARRIER: u32 = 224;
const OP_LOOP_MERGE: u32 = 246;
const OP_SELECTION_MERGE: u32 = 247;
const OP_LABEL: u32 = 248;
const OP_BRANCH: u32 = 249;
const OP_BRANCH_CONDITIONAL: u32 = 250;
const OP_RETURN: u32 = 253;
const OP_EMIT_MESH_TASKS: u32 = 5294;
const OP_SET_MESH_OUTPUTS: u32 = 5295;

const SC_INPUT: u32 = 1;
const SC_OUTPUT: u32 = 3;
const SC_WORKGROUP: u32 = 4;
const SC_FUNCTION: u32 = 7;
const SC_TASK_PAYLOAD: u32 = 5402;

const DEC_BUILTIN: u32 = 11;
const DEC_LOCATION: u32 = 30;
const DEC_PER_PRIMITIVE: u32 = 5271;

const BI_POSITION: u32 = 0;
const BI_POINT_SIZE: u32 = 1;
const BI_PRIMITIVE_ID: u32 = 7;
const BI_LAYER: u32 = 9;
const BI_VIEWPORT_INDEX: u32 = 10;
const BI_WORKGROUP_ID: u32 = 26;
const BI_LOCAL_INVOCATION_INDEX: u32 = 29;
const BI_SUBGROUP_LOCAL_ID: u32 = 41;
const BI_POINT_INDICES: u32 = 5294;
const BI_LINE_INDICES: u32 = 5295;
const BI_TRIANGLE_INDICES: u32 = 5296;

const EM_OUTPUT_VERTICES: u32 = 26;
const EM_OUTPUT_POINTS: u32 = 27;
const EM_OUTPUT_LINES: u32 = 5269;
const EM_OUTPUT_PRIMITIVES: u32 = 5270;
const EM_OUTPUT_TRIANGLES: u32 = 5298;
const CAP_MESH_SHADING: u32 = 5283;
const MODEL_TASK: u32 = 5364;
const MODEL_MESH: u32 = 5365;

const BLOCK: u32 = 32;
/// Mesh workgroups a task workgroup launches at most (Vulkan's guaranteed maxMeshWorkGroupCount[0]).
const MAX_LAUNCH: u32 = 65535;

/// The facts the pass needs, from job.rs's `mesh_stage` info.
#[derive(Debug, Clone)]
pub struct Info {
    pub task: bool, // kind "ts"
    pub threads: u32,
    pub payload: u32,
    pub has_task: bool,
    pub verts: u32,
    pub max_v: u32,
    pub max_p: u32,
    /// (attribute: -2 point size, -1 position, n generic; component)
    pub vertex: Vec<(i64, u32)>,
    pub primitive: Vec<(i64, u32)>,
    pub primitive_system: Vec<u32>,
    /// byte offsets: workgroup word, input, outputs, primitive bytes
    pub memory: Vec<u32>,
}

impl Info {
    pub fn from_json(v: &Value) -> Result<Info, String> {
        let u = |k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0) as u32;
        let comps = |k: &str| -> Vec<(i64, u32)> {
            v.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(|c| Some((c.get(0)?.as_i64()?, c.get(1)?.as_u64()? as u32))).collect()).unwrap_or_default()
        };
        let kind = v.get("kind").and_then(Value::as_str).ok_or("mesh info without a kind")?;
        let memory: Vec<u32> = v.get("memory").and_then(Value::as_array).ok_or("mesh info without its memory layout")?.iter().filter_map(|x| x.as_u64().map(|x| x as u32)).collect();
        if memory.len() < 4 {
            return Err("mesh info: memory layout too short".into());
        }
        Ok(Info {
            task: kind == "ts",
            threads: u("threads").max(1),
            payload: u("payload"),
            has_task: v.get("task").and_then(Value::as_bool).unwrap_or(false),
            verts: match u("verts") { 0 => 3, n => n },
            max_v: u("max_v"),
            max_p: u("max_p"),
            vertex: comps("vertex"),
            primitive: comps("primitive"),
            primitive_system: v.get("primitive_system").and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_u64().map(|x| x as u32)).collect()).unwrap_or_default(),
            memory,
        })
    }
}

fn str_words(s: &str) -> Vec<u32> {
    let mut b = s.as_bytes().to_vec();
    b.push(0);
    while b.len() % 4 != 0 {
        b.push(0);
    }
    b.chunks(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect()
}

fn inst(op: u32, ops: &[u32]) -> Vec<u32> {
    let mut v = Vec::with_capacity(ops.len() + 1);
    v.push(((ops.len() as u32 + 1) << 16) | op);
    v.extend_from_slice(ops);
    v
}

struct M {
    head: [u32; 5],
    ins: Vec<Vec<u32>>,
}

impl M {
    fn parse(w: &[u32]) -> Option<M> {
        if w.len() < 5 {
            return None;
        }
        let mut ins = vec![];
        let mut i = 5;
        while i < w.len() {
            let n = (w[i] >> 16) as usize;
            if n == 0 || i + n > w.len() {
                return None;
            }
            ins.push(w[i..i + n].to_vec());
            i += n;
        }
        Some(M { head: w[..5].try_into().unwrap(), ins })
    }
    fn words(&self) -> Vec<u32> {
        let mut v = self.head.to_vec();
        for i in &self.ins {
            v.extend_from_slice(i);
        }
        v
    }
    fn id(&mut self) -> u32 {
        let id = self.head[3];
        self.head[3] += 1;
        id
    }
    fn op(i: &[u32]) -> u32 {
        i[0] & 0xffff
    }
    fn first_function(&self) -> usize {
        self.ins.iter().position(|i| M::op(i) == OP_FUNCTION).unwrap_or(self.ins.len())
    }
    /// A type with these operands (after its result id), made at the end of the global section when missing.
    fn ty(&mut self, op: u32, ops: &[u32]) -> u32 {
        if let Some(i) = self.ins.iter().find(|i| M::op(i) == op && i.len() == ops.len() + 2 && &i[2..] == ops) {
            return i[1];
        }
        let id = self.id();
        let mut v = vec![id];
        v.extend_from_slice(ops);
        let at = self.first_function();
        self.ins.insert(at, inst(op, &v));
        id
    }
    fn constant(&mut self, ty: u32, val: u32) -> u32 {
        if let Some(i) = self.ins.iter().find(|i| M::op(i) == OP_CONSTANT && i.len() == 4 && i[1] == ty && i[3] == val) {
            return i[2];
        }
        let id = self.id();
        let at = self.first_function();
        self.ins.insert(at, inst(OP_CONSTANT, &[ty, id, val]));
        id
    }
    fn global_var(&mut self, ptr: u32, sc: u32) -> u32 {
        let id = self.id();
        let at = self.first_function();
        self.ins.insert(at, inst(OP_VARIABLE, &[ptr, id, sc]));
        id
    }
    fn decorate(&mut self, d: &[u32]) {
        let at = self
            .ins
            .iter()
            .rposition(|i| matches!(M::op(i), 71 | 72 | 73 | 74 | 75 | 332 | 5632 | 5633))
            .map(|p| p + 1)
            .unwrap_or_else(|| self.ins.iter().position(|i| matches!(M::op(i), 19..=39 | 43..=52 | 59)).unwrap_or(0));
        self.ins.insert(at, inst(OP_DECORATE, d));
    }
    fn builtin_var(&self, b: u32) -> Option<u32> {
        self.ins.iter().find(|i| M::op(i) == OP_DECORATE && i.len() >= 4 && i[2] == DEC_BUILTIN && i[3] == b).map(|i| i[1])
    }
    fn def(&self, id: u32) -> Option<&Vec<u32>> {
        self.ins.iter().find(|i| {
            let op = M::op(i);
            match op {
                19..=39 => i.get(1) == Some(&id),
                _ => i.len() > 2 && i[2] == id && matches!(op, 43..=52 | 59),
            }
        })
    }
}

/// Function body emission with its own ids.
struct F<'a> {
    m: &'a mut M,
    code: Vec<Vec<u32>>,
    vars: Vec<Vec<u32>>,
}

impl F<'_> {
    fn e(&mut self, op: u32, ty: u32, ops: &[u32]) -> u32 {
        let id = self.m.id();
        let mut v = vec![ty, id];
        v.extend_from_slice(ops);
        self.code.push(inst(op, &v));
        id
    }
    fn raw(&mut self, op: u32, ops: &[u32]) {
        self.code.push(inst(op, ops));
    }
    fn label(&mut self, l: u32) {
        self.raw(OP_LABEL, &[l]);
    }
    fn local(&mut self, ptr: u32) -> u32 {
        let id = self.m.id();
        self.vars.push(inst(OP_VARIABLE, &[ptr, id, SC_FUNCTION]));
        id
    }
    /// for (n = start; n < end; n += step) body(n)
    fn for_range(&mut self, var: u32, uint: u32, boolt: u32, start: u32, end: u32, step: u32, body: &mut dyn FnMut(&mut F, u32)) {
        let (head, cond, bl, cont, merge) = (self.m.id(), self.m.id(), self.m.id(), self.m.id(), self.m.id());
        self.raw(OP_STORE, &[var, start]);
        self.raw(OP_BRANCH, &[head]);
        self.label(head);
        self.raw(OP_LOOP_MERGE, &[merge, cont, 0]);
        self.raw(OP_BRANCH, &[cond]);
        self.label(cond);
        let n = self.e(OP_LOAD, uint, &[var]);
        let c = self.e(OP_ULESS_THAN, boolt, &[n, end]);
        self.raw(OP_BRANCH_CONDITIONAL, &[c, bl, merge]);
        self.label(bl);
        body(self, n);
        self.raw(OP_BRANCH, &[cont]);
        self.label(cont);
        let n2 = self.e(OP_LOAD, uint, &[var]);
        let n3 = self.e(OP_IADD, uint, &[n2, step]);
        self.raw(OP_STORE, &[var, n3]);
        self.raw(OP_BRANCH, &[head]);
        self.label(merge);
    }
    fn if_then(&mut self, c: u32, body: &mut dyn FnMut(&mut F)) {
        let (t, merge) = (self.m.id(), self.m.id());
        self.raw(OP_SELECTION_MERGE, &[merge, 0]);
        self.raw(OP_BRANCH_CONDITIONAL, &[c, t, merge]);
        self.label(t);
        body(self);
        self.raw(OP_BRANCH, &[merge]);
        self.label(merge);
    }
}

/// The translated compute kernel → the task or mesh stage. Errors say why it cannot be.
pub fn to_stage(spv: &[u8], i: &Info) -> Result<Vec<u8>, String> {
    let w = crate::reflect::words(spv).ok_or("not SPIR-V")?;
    let mut m = M::parse(&w).ok_or("SPIR-V that does not parse")?;
    let kind = if i.task { "task" } else { "mesh" };
    // the kernel and its stage memory
    let ep = m.ins.iter().position(|x| M::op(x) == OP_ENTRY_POINT).ok_or("no entry point")?;
    let old = m.ins[ep][2];
    let wvars: Vec<(u32, u32)> = m.ins.iter().filter(|x| M::op(x) == OP_VARIABLE && x.len() >= 4 && x[3] == SC_WORKGROUP).map(|x| (x[2], x[1])).collect();
    if wvars.len() != 1 {
        return Err(format!("{kind}: {} workgroup variables (the stage memory must be the only one)", wvars.len()));
    }
    let (mem, memptr) = wvars[0];
    let arr = m.def(memptr).and_then(|p| p.get(3).copied()).ok_or("stage memory pointer type")?;
    let (elem, len_id) = m.def(arr).filter(|a| M::op(a) == OP_TYPE_ARRAY).map(|a| (a[2], a[3])).ok_or("stage memory is not an array")?;
    let words = m.def(len_id).filter(|c| M::op(c) == OP_CONSTANT).map(|c| c[3]).ok_or("stage memory length")?;
    if !m.def(elem).is_some_and(|t| M::op(t) == OP_TYPE_INT && t[2] == 32) {
        return Err(format!("{kind}: stage memory of other than 32-bit words"));
    }
    let lay = &i.memory;
    let (wg, inp, out, prim) = (lay[0] / 4, lay[1] / 4, lay[2] / 4, lay[3]);

    // types
    let voidt = m.ty(OP_TYPE_VOID, &[]);
    let boolt = m.ty(OP_TYPE_BOOL, &[]);
    let uint = elem;
    let flt = m.ty(OP_TYPE_FLOAT, &[32]);
    let vec4 = m.ty(OP_TYPE_VECTOR, &[flt, 4]);
    let uvec3 = m.ty(OP_TYPE_VECTOR, &[uint, 3]);
    let fnt = m.ty(OP_TYPE_FUNCTION, &[voidt]);
    let memp = m.ty(OP_TYPE_POINTER, &[SC_WORKGROUP, uint]);
    let fnp = m.ty(OP_TYPE_POINTER, &[SC_FUNCTION, uint]);
    let in_u = m.ty(OP_TYPE_POINTER, &[SC_INPUT, uint]);
    let in_u3 = m.ty(OP_TYPE_POINTER, &[SC_INPUT, uvec3]);
    let mut consts = std::collections::HashMap::new();
    let mut c = |m: &mut M, v: u32| *consts.entry(v).or_insert_with(|| m.constant(uint, v));

    // the invocation's index and the workgroup's number
    let lid = match m.builtin_var(BI_LOCAL_INVOCATION_INDEX) {
        Some(v) => v,
        None => match m.builtin_var(BI_SUBGROUP_LOCAL_ID) {
            Some(v) => {
                for x in m.ins.iter_mut() {
                    if M::op(x) == OP_DECORATE && x.len() >= 4 && x[1] == v && x[2] == DEC_BUILTIN && x[3] == BI_SUBGROUP_LOCAL_ID {
                        x[3] = BI_LOCAL_INVOCATION_INDEX;
                    }
                }
                v
            }
            None => {
                let v = m.global_var(in_u, SC_INPUT);
                m.decorate(&[v, DEC_BUILTIN, BI_LOCAL_INVOCATION_INDEX]);
                v
            }
        },
    };
    let wid = match m.builtin_var(BI_WORKGROUP_ID) {
        Some(v) => v,
        None => {
            let v = m.global_var(in_u3, SC_INPUT);
            m.decorate(&[v, DEC_BUILTIN, BI_WORKGROUP_ID]);
            v
        }
    };
    let pay_words = (i.payload + 3) / 4;
    let pay_words = pay_words.max(1);
    let payload = if i.task || i.has_task {
        let n = c(&mut m, pay_words);
        let pt = m.ty(OP_TYPE_ARRAY, &[uint, n]);
        let pp = m.ty(OP_TYPE_POINTER, &[SC_TASK_PAYLOAD, pt]);
        Some(m.global_var(pp, SC_TASK_PAYLOAD))
    } else {
        None
    };
    let pay_e = m.ty(OP_TYPE_POINTER, &[SC_TASK_PAYLOAD, uint]);

    // a mesh shader's outputs: (variable, element pointer, per attribute the component indices)
    struct Out {
        var: u32,
        elem_ptr: u32,
        comps: [Option<u32>; 4],
        scalar: bool,
    }
    let mut vouts: Vec<Out> = vec![];
    let mut pouts: Vec<Out> = vec![]; // per-primitive generic attributes written at the primitive's vertices
    let mut sys_outs: Vec<(u32, u32, u32)> = vec![]; // (variable, element pointer, component in the primitive block)
    let mut idx_var = (0, 0, 0); // (variable, element pointer, index type)
    let (nv, npc) = (i.vertex.len() as u32, i.primitive_system.len() as u32 + i.primitive.len() as u32);
    if !i.task {
        if i.max_v == 0 || i.max_p == 0 {
            return Err("mesh: no vertices or primitives".into());
        }
        if !i.vertex.iter().any(|v| v.0 == -1) {
            return Err("mesh: its SPH enables no position component".into());
        }
        let nmax_v = c(&mut m, i.max_v);
        let nmax_p = c(&mut m, i.max_p);
        let va4 = m.ty(OP_TYPE_ARRAY, &[vec4, nmax_v]);
        let va4p = m.ty(OP_TYPE_POINTER, &[SC_OUTPUT, va4]);
        let o4 = m.ty(OP_TYPE_POINTER, &[SC_OUTPUT, vec4]);
        let mut attrs: Vec<(i64, [Option<u32>; 4])> = vec![];
        for (k, &(a, comp)) in i.vertex.iter().enumerate() {
            match attrs.iter_mut().find(|x| x.0 == a) {
                Some(x) => x.1[comp as usize & 3] = Some(k as u32),
                None => {
                    let mut cs = [None; 4];
                    cs[comp as usize & 3] = Some(k as u32);
                    attrs.push((a, cs));
                }
            }
        }
        for (a, cs) in &attrs {
            if *a == -2 {
                let fa = m.ty(OP_TYPE_ARRAY, &[flt, nmax_v]);
                let fap = m.ty(OP_TYPE_POINTER, &[SC_OUTPUT, fa]);
                let of = m.ty(OP_TYPE_POINTER, &[SC_OUTPUT, flt]);
                let v = m.global_var(fap, SC_OUTPUT);
                m.decorate(&[v, DEC_BUILTIN, BI_POINT_SIZE]);
                vouts.push(Out { var: v, elem_ptr: of, comps: *cs, scalar: true });
                continue;
            }
            let v = m.global_var(va4p, SC_OUTPUT);
            if *a == -1 {
                m.decorate(&[v, DEC_BUILTIN, BI_POSITION]);
            } else {
                m.decorate(&[v, DEC_LOCATION, *a as u32]);
            }
            vouts.push(Out { var: v, elem_ptr: o4, comps: *cs, scalar: false });
        }
        let mut pattrs: Vec<(i64, [Option<u32>; 4])> = vec![];
        for (k, &(a, comp)) in i.primitive.iter().enumerate() {
            let at = i.primitive_system.len() as u32 + k as u32;
            match pattrs.iter_mut().find(|x| x.0 == a) {
                Some(x) => x.1[comp as usize & 3] = Some(at),
                None => {
                    let mut cs = [None; 4];
                    cs[comp as usize & 3] = Some(at);
                    pattrs.push((a, cs));
                }
            }
        }
        for (a, cs) in pattrs {
            if attrs.iter().any(|x| x.0 == a) || a < 0 {
                continue;
            }
            let v = m.global_var(va4p, SC_OUTPUT);
            m.decorate(&[v, DEC_LOCATION, a as u32]);
            pouts.push(Out { var: v, elem_ptr: o4, comps: cs, scalar: false });
        }
        // per-primitive system values: OMAP bit 8 the primitive ID, 9 the layer, 10 the viewport
        let ua = m.ty(OP_TYPE_ARRAY, &[uint, nmax_p]);
        let uap = m.ty(OP_TYPE_POINTER, &[SC_OUTPUT, ua]);
        let ou = m.ty(OP_TYPE_POINTER, &[SC_OUTPUT, uint]);
        for (k, &bit) in i.primitive_system.iter().enumerate() {
            let b = match bit {
                8 => BI_PRIMITIVE_ID,
                9 => BI_LAYER,
                10 => BI_VIEWPORT_INDEX,
                _ => continue,
            };
            let v = m.global_var(uap, SC_OUTPUT);
            m.decorate(&[v, DEC_BUILTIN, b]);
            m.decorate(&[v, DEC_PER_PRIMITIVE]);
            sys_outs.push((v, ou, k as u32));
        }
        let (it, bi) = match i.verts {
            1 => (uint, BI_POINT_INDICES),
            2 => (m.ty(OP_TYPE_VECTOR, &[uint, 2]), BI_LINE_INDICES),
            _ => (uvec3, BI_TRIANGLE_INDICES),
        };
        let ia = m.ty(OP_TYPE_ARRAY, &[it, nmax_p]);
        let iap = m.ty(OP_TYPE_POINTER, &[SC_OUTPUT, ia]);
        let ie = m.ty(OP_TYPE_POINTER, &[SC_OUTPUT, it]);
        let v = m.global_var(iap, SC_OUTPUT);
        m.decorate(&[v, DEC_BUILTIN, bi]);
        idx_var = (v, ie, it);
    }
    let f0 = {
        let z = m.constant(flt, 0f32.to_bits());
        z
    };
    let f1 = m.constant(flt, 1f32.to_bits());

    // SPIR-V 1.4, the capability and the extension
    m.head[1] = m.head[1].max(0x0001_0400);
    let last_cap = m.ins.iter().rposition(|x| M::op(x) == OP_CAPABILITY).ok_or("no capability")?;
    m.ins.insert(last_cap + 1, inst(OP_CAPABILITY, &[CAP_MESH_SHADING]));
    let after_caps = m.ins.iter().rposition(|x| M::op(x) == OP_CAPABILITY).unwrap() + 1;
    let mut ext = vec![];
    ext.extend(str_words("SPV_EXT_mesh_shader"));
    m.ins.insert(after_caps, inst(OP_EXTENSION, &ext));

    // the entry function
    let entry = m.id();
    let threads = c(&mut m, i.threads);
    let zero = c(&mut m, 0);
    let one = c(&mut m, 1);
    let nwords = c(&mut m, words);
    let k2 = c(&mut m, 2);
    let k0x108 = c(&mut m, 0x108);
    let cwg = c(&mut m, wg);
    let cinp = c(&mut m, inp);
    let cout = c(&mut m, out);
    let cout1 = c(&mut m, out + 1);
    let cpay = c(&mut m, pay_words);
    let climit = c(&mut m, MAX_LAUNCH);
    let cblock = c(&mut m, BLOCK);
    let (cmaxv, cmaxp) = (c(&mut m, i.max_v.max(1)), c(&mut m, i.max_p.max(1)));
    let cvstride = c(&mut m, BLOCK * nv.max(1));
    let cpstride = c(&mut m, BLOCK * npc.max(1));
    let cpbase = c(&mut m, out + (i.max_v.max(1) + BLOCK - 1) / BLOCK * BLOCK * nv);
    let cprimw = c(&mut m, prim / 4);
    let c24 = c(&mut m, 24);
    let cidx0 = c(&mut m, prim + 4);
    let cverts = c(&mut m, i.verts);
    let c3 = c(&mut m, 3);
    let c8 = c(&mut m, 8);
    let c255 = c(&mut m, 255);
    let mut kconst: Vec<u32> = vec![];
    for k in 0..(nv.max(npc) + 1) {
        kconst.push(c(&mut m, k * BLOCK));
    }

    let mut f = F { m: &mut m, code: vec![], vars: vec![] };
    let ivar = f.local(fnp);
    let lv = f.e(OP_LOAD, uint, &[lid]);
    let w3 = f.e(OP_LOAD, uvec3, &[wid]);
    let tg = f.e(OP_COMPOSITE_EXTRACT, uint, &[w3, 0]);
    let is0 = f.e(OP_IEQUAL, boolt, &[lv, zero]);
    let barrier = |f: &mut F| f.raw(OP_CONTROL_BARRIER, &[k2, k2, k0x108]);
    let store_mem = |f: &mut F, idx: u32, val: u32| {
        let p = f.e(OP_ACCESS_CHAIN, memp, &[mem, idx]);
        f.raw(OP_STORE, &[p, val]);
    };
    let load_mem = |f: &mut F, idx: u32| {
        let p = f.e(OP_ACCESS_CHAIN, memp, &[mem, idx]);
        f.e(OP_LOAD, uint, &[p])
    };
    // 1. the stage memory starts at zero
    f.for_range(ivar, uint, boolt, lv, nwords, threads, &mut |f, n| store_mem(f, n, zero));
    barrier(&mut f);
    if let (false, Some(pv)) = (i.task, payload) {
        f.for_range(ivar, uint, boolt, lv, cpay, threads, &mut |f, n| {
            let pp = f.e(OP_ACCESS_CHAIN, pay_e, &[pv, n]);
            let v = f.e(OP_LOAD, uint, &[pp]);
            let at = f.e(OP_IADD, uint, &[cinp, n]);
            store_mem(f, at, v);
        });
        barrier(&mut f);
        f.if_then(is0, &mut |f| {
            store_mem(f, cwg, tg);
            store_mem(f, cinp, tg);
        });
    } else {
        f.if_then(is0, &mut |f| store_mem(f, cwg, tg));
    }
    barrier(&mut f);
    // 2. the kernel
    let call = f.m.id();
    f.raw(OP_FUNCTION_CALL, &[voidt, call, old]);
    barrier(&mut f);
    // 3. the stage's outputs
    if i.task {
        let pv = payload.unwrap();
        f.for_range(ivar, uint, boolt, lv, cpay, threads, &mut |f, n| {
            let at = f.e(OP_IADD, uint, &[cout, n]);
            let v = load_mem(f, at);
            let pp = f.e(OP_ACCESS_CHAIN, pay_e, &[pv, n]);
            f.raw(OP_STORE, &[pp, v]);
        });
        barrier(&mut f);
        let want = load_mem(&mut f, cout1);
        let lt = f.e(OP_ULESS_THAN, boolt, &[want, climit]);
        let count = f.e(OP_SELECT, uint, &[lt, want, climit]);
        f.raw(OP_EMIT_MESH_TASKS, &[count, one, one, pv]);
    } else {
        let pw = load_mem(&mut f, cprimw);
        let pc = f.e(OP_SHIFT_RIGHT_LOGICAL, uint, &[pw, c24]);
        let lt = f.e(OP_ULESS_THAN, boolt, &[pc, cmaxp]);
        let np = f.e(OP_SELECT, uint, &[lt, pc, cmaxp]);
        f.raw(OP_SET_MESH_OUTPUTS, &[cmaxv, np]);
        // the word of component k of element n of a block area starting at `base`, `stride` words a block
        let word = |f: &mut F, base: u32, stride: u32, n: u32, k: u32| {
            let blk = f.e(OP_UDIV, uint, &[n, cblock]);
            let bo = f.e(OP_IMUL, uint, &[blk, stride]);
            let lane = f.e(OP_UMOD, uint, &[n, cblock]);
            let a = f.e(OP_IADD, uint, &[base, bo]);
            let a = f.e(OP_IADD, uint, &[a, lane]);
            let a = f.e(OP_IADD, uint, &[a, kconst[k as usize]]);
            load_mem(f, a)
        };
        let vec4_of = |f: &mut F, base: u32, stride: u32, n: u32, cs: &[Option<u32>; 4]| {
            let mut v = [0u32; 4];
            for q in 0..4 {
                v[q] = match cs[q] {
                    Some(k) => {
                        let x = word(f, base, stride, n, k);
                        f.e(OP_BITCAST, flt, &[x])
                    }
                    None => {
                        if q == 3 {
                            f1
                        } else {
                            f0
                        }
                    }
                };
            }
            f.e(OP_COMPOSITE_CONSTRUCT, vec4, &v)
        };
        f.for_range(ivar, uint, boolt, lv, cmaxv, threads, &mut |f, n| {
            for o in &vouts {
                let val = if o.scalar {
                    let x = word(f, cout, cvstride, n, o.comps.iter().flatten().next().copied().unwrap_or(0));
                    f.e(OP_BITCAST, flt, &[x])
                } else {
                    vec4_of(f, cout, cvstride, n, &o.comps)
                };
                let p = f.e(OP_ACCESS_CHAIN, o.elem_ptr, &[o.var, n]);
                f.raw(OP_STORE, &[p, val]);
            }
        });
        let (iv, ie, it) = idx_var;
        f.for_range(ivar, uint, boolt, lv, np, threads, &mut |f, n| {
            let mut idx = vec![];
            for q in 0..i.verts.min(3) {
                let cq = kconst_small(f, q);
                let nv3 = f.e(OP_IMUL, uint, &[n, cverts]);
                let at = f.e(OP_IADD, uint, &[cidx0, nv3]);
                let at = f.e(OP_IADD, uint, &[at, cq]);
                let wi = f.e(OP_SHIFT_RIGHT_LOGICAL, uint, &[at, k2]);
                let wv = load_mem(f, wi);
                let sh = f.e(OP_BITWISE_AND, uint, &[at, c3]);
                let sh = f.e(OP_IMUL, uint, &[sh, c8]);
                let b = f.e(OP_SHIFT_RIGHT_LOGICAL, uint, &[wv, sh]);
                idx.push(f.e(OP_BITWISE_AND, uint, &[b, c255]));
            }
            let val = if idx.len() == 1 { idx[0] } else { f.e(OP_COMPOSITE_CONSTRUCT, it, &idx) };
            let p = f.e(OP_ACCESS_CHAIN, ie, &[iv, n]);
            f.raw(OP_STORE, &[p, val]);
            for &(sv, se, k) in &sys_outs {
                let x = word(f, cpbase, cpstride, n, k);
                let p = f.e(OP_ACCESS_CHAIN, se, &[sv, n]);
                f.raw(OP_STORE, &[p, x]);
            }
            for o in &pouts {
                let val = vec4_of(f, cpbase, cpstride, n, &o.comps);
                for &vi in &idx {
                    let p = f.e(OP_ACCESS_CHAIN, o.elem_ptr, &[o.var, vi]);
                    f.raw(OP_STORE, &[p, val]);
                }
            }
        });
        f.raw(OP_RETURN, &[]);
    }
    let (code, vars) = (std::mem::take(&mut f.code), std::mem::take(&mut f.vars));
    let first_label = m.id();
    let mut func = vec![inst(OP_FUNCTION, &[voidt, entry, 0, fnt]), inst(OP_LABEL, &[first_label])];
    func.extend(vars);
    func.extend(code);
    func.push(inst(OP_FUNCTION_END, &[]));
    m.ins.extend(func);

    // the entry point: the new function, every global variable in its interface; its modes
    let ep = m.ins.iter().position(|x| M::op(x) == OP_ENTRY_POINT).unwrap();
    let name_len = {
        let e = &m.ins[ep];
        let mut n = 0;
        for (k, wd) in e[3..].iter().enumerate() {
            if wd.to_le_bytes().contains(&0) {
                n = k + 1;
                break;
            }
        }
        n
    };
    let name: Vec<u32> = m.ins[ep][3..3 + name_len].to_vec();
    let ff = m.first_function();
    let mut iface: Vec<u32> = vec![];
    for x in &m.ins[..ff] {
        if M::op(x) == OP_VARIABLE && x.len() >= 4 && x[3] != SC_FUNCTION && !iface.contains(&x[2]) {
            iface.push(x[2]);
        }
    }
    let mut e = vec![if i.task { MODEL_TASK } else { MODEL_MESH }, entry];
    e.extend(name);
    e.extend(iface);
    m.ins[ep] = inst(OP_ENTRY_POINT, &e);
    for x in m.ins.iter_mut() {
        if M::op(x) == OP_EXECUTION_MODE && x[1] == old {
            x[1] = entry;
        }
    }
    if !i.task {
        let at = m.ins.iter().rposition(|x| M::op(x) == OP_EXECUTION_MODE).map(|p| p + 1).unwrap_or(ep + 1);
        let topo = match i.verts {
            1 => EM_OUTPUT_POINTS,
            2 => EM_OUTPUT_LINES,
            _ => EM_OUTPUT_TRIANGLES,
        };
        m.ins.insert(at, inst(OP_EXECUTION_MODE, &[entry, topo]));
        m.ins.insert(at, inst(OP_EXECUTION_MODE, &[entry, EM_OUTPUT_PRIMITIVES, i.max_p]));
        m.ins.insert(at, inst(OP_EXECUTION_MODE, &[entry, EM_OUTPUT_VERTICES, i.max_v]));
    }
    let _ = (OP_MEMORY_MODEL, OP_CONSTANT_NULL);
    Ok(m.words().iter().flat_map(|x| x.to_le_bytes()).collect())
}

/// A small uint constant inside a function's emission (made in the module's global section).
fn kconst_small(f: &mut F, v: u32) -> u32 {
    let uint = f.m.ins.iter().find(|i| M::op(i) == OP_TYPE_INT && i.len() == 4 && i[2] == 32 && i[3] == 0).map(|i| i[1]).unwrap();
    f.m.constant(uint, v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_are_nul_terminated_words() {
        assert_eq!(str_words("abc"), vec![u32::from_le_bytes(*b"abc\0")]);
        assert_eq!(str_words("abcd").len(), 2);
    }
}
