//! The descriptor bindings a SPIR-V module declares (set, binding, Vulkan descriptor type, array count), read from
//! its decorations and types, so the graphics module creates its descriptor set layouts without guessing; and the
//! module's execution model and capabilities.

use serde_json::{json, Value};
use std::collections::HashMap;

const OP_CAPABILITY: u32 = 17;
const OP_ENTRY_POINT: u32 = 15;
const OP_EXECUTION_MODE: u32 = 16;
const OP_TYPE_IMAGE: u32 = 25;
const OP_TYPE_SAMPLER: u32 = 26;
const OP_TYPE_SAMPLED_IMAGE: u32 = 27;
const OP_TYPE_ARRAY: u32 = 28;
const OP_TYPE_RUNTIME_ARRAY: u32 = 29;
const OP_TYPE_STRUCT: u32 = 30;
const OP_TYPE_POINTER: u32 = 32;
const OP_TYPE_ACCELERATION_STRUCTURE: u32 = 5341;
const OP_CONSTANT: u32 = 43;
const OP_VARIABLE: u32 = 59;
const OP_DECORATE: u32 = 71;
const OP_NAME: u32 = 5;
const DEC_BLOCK: u32 = 2;
const DEC_BUFFER_BLOCK: u32 = 3;
const DEC_BINDING: u32 = 33;
const DEC_SET: u32 = 34;
const SC_UNIFORM_CONSTANT: u32 = 0;
const SC_UNIFORM: u32 = 2;
const SC_PUSH_CONSTANT: u32 = 9;
const SC_STORAGE_BUFFER: u32 = 12;
const DIM_BUFFER: u32 = 5;
const DIM_SUBPASS: u32 = 6;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Binding {
    pub set: u32,
    pub binding: u32,
    /// Vulkan's name without the prefix: uniform_buffer, storage_buffer, combined_image_sampler, sampled_image,
    /// sampler, storage_image, uniform_texel_buffer, storage_texel_buffer, input_attachment.
    pub kind: &'static str,
    /// Array elements; 0 = a runtime-sized array (descriptor indexing).
    pub count: u32,
    pub name: String,
}

#[derive(Debug, Default)]
pub struct Module {
    pub version: (u32, u32),
    pub execution_model: Option<&'static str>,
    pub capabilities: Vec<u32>,
    pub bindings: Vec<Binding>,
    pub push_constants: bool,
    pub local_size: Option<[u32; 3]>,
}

fn model_name(m: u32) -> &'static str {
    match m {
        0 => "vertex",
        1 => "tess_control",
        2 => "tess_eval",
        3 => "geometry",
        4 => "fragment",
        5 => "compute",
        5267 => "task",
        5268 => "mesh",
        5364 => "task",
        5365 => "mesh",
        _ => "other",
    }
}

pub fn words(spv: &[u8]) -> Option<Vec<u32>> {
    if spv.len() % 4 != 0 || spv.len() < 20 {
        return None;
    }
    let w: Vec<u32> = spv
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
        .collect();
    (w[0] == 0x0723_0203).then_some(w)
}

pub fn reflect(spv: &[u8]) -> Result<Module, String> {
    let w = words(spv).ok_or("not a SPIR-V module")?;
    let mut m = Module {
        version: ((w[1] >> 16) & 0xff, (w[1] >> 8) & 0xff),
        ..Default::default()
    };
    let mut set: HashMap<u32, u32> = HashMap::new();
    let mut binding: HashMap<u32, u32> = HashMap::new();
    let mut block: HashMap<u32, u32> = HashMap::new();
    let mut names: HashMap<u32, String> = HashMap::new();
    // type id -> instruction words
    let mut types: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut consts: HashMap<u32, u32> = HashMap::new();
    let mut vars: Vec<(u32, u32, u32)> = vec![]; // (id, pointer type, storage class)
    let mut entry = None;
    let mut i = 5;
    while i < w.len() {
        let (n, op) = ((w[i] >> 16) as usize, w[i] & 0xffff);
        if n == 0 || i + n > w.len() {
            return Err(format!("damaged instruction at word {i}"));
        }
        let ins = &w[i..i + n];
        match op {
            OP_CAPABILITY => m.capabilities.push(ins[1]),
            OP_ENTRY_POINT => {
                m.execution_model = Some(model_name(ins[1]));
                entry = Some(ins[2]);
            }
            OP_EXECUTION_MODE if n >= 6 && ins[2] == 17 && Some(ins[1]) == entry => {
                m.local_size = Some([ins[3], ins[4], ins[5]])
            }
            OP_NAME if n >= 3 => {
                let b: Vec<u8> = ins[2..].iter().flat_map(|x| x.to_le_bytes()).collect();
                let z = b.iter().position(|&c| c == 0).unwrap_or(b.len());
                names.insert(ins[1], String::from_utf8_lossy(&b[..z]).into_owned());
            }
            OP_DECORATE if n >= 3 => match ins[2] {
                DEC_SET if n >= 4 => {
                    set.insert(ins[1], ins[3]);
                }
                DEC_BINDING if n >= 4 => {
                    binding.insert(ins[1], ins[3]);
                }
                DEC_BLOCK | DEC_BUFFER_BLOCK => {
                    block.insert(ins[1], ins[2]);
                }
                _ => {}
            },
            OP_TYPE_IMAGE
            | OP_TYPE_SAMPLER
            | OP_TYPE_SAMPLED_IMAGE
            | OP_TYPE_ARRAY
            | OP_TYPE_RUNTIME_ARRAY
            | OP_TYPE_STRUCT
            | OP_TYPE_POINTER
            | OP_TYPE_ACCELERATION_STRUCTURE => {
                types.insert(ins[1], ins.to_vec());
            }
            OP_CONSTANT if n >= 4 => {
                consts.insert(ins[2], ins[3]);
            }
            OP_VARIABLE if n >= 4 => vars.push((ins[2], ins[1], ins[3])),
            _ => {}
        }
        i += n;
    }
    for (id, ptr, sc) in vars {
        if sc == SC_PUSH_CONSTANT {
            m.push_constants = true;
        }
        let (Some(&s), Some(&b)) = (set.get(&id), binding.get(&id)) else {
            continue;
        };
        let mut t = types
            .get(&ptr)
            .filter(|t| t[0] & 0xffff == OP_TYPE_POINTER)
            .map(|t| t[3])
            .ok_or_else(|| format!("variable {id}: not a pointer"))?;
        let mut count = 1u32;
        loop {
            match types.get(&t) {
                Some(ins) if ins[0] & 0xffff == OP_TYPE_ARRAY => {
                    count *= consts.get(&ins[3]).copied().unwrap_or(1);
                    t = ins[2];
                }
                Some(ins) if ins[0] & 0xffff == OP_TYPE_RUNTIME_ARRAY => {
                    count = 0;
                    t = ins[2];
                }
                _ => break,
            }
        }
        let ins = types.get(&t);
        let kind = match (sc, ins.map(|x| x[0] & 0xffff)) {
            (SC_STORAGE_BUFFER, _) => "storage_buffer",
            (SC_UNIFORM, Some(OP_TYPE_STRUCT)) if block.get(&t) == Some(&DEC_BUFFER_BLOCK) => {
                "storage_buffer"
            }
            (SC_UNIFORM, _) => "uniform_buffer",
            (SC_UNIFORM_CONSTANT, Some(OP_TYPE_SAMPLER)) => "sampler",
            (SC_UNIFORM_CONSTANT, Some(OP_TYPE_ACCELERATION_STRUCTURE)) => "acceleration_structure",
            (SC_UNIFORM_CONSTANT, Some(OP_TYPE_SAMPLED_IMAGE)) => {
                let img = types.get(&ins.unwrap()[2]);
                if img.is_some_and(|im| im[3] == DIM_BUFFER) {
                    "uniform_texel_buffer"
                } else {
                    "combined_image_sampler"
                }
            }
            (SC_UNIFORM_CONSTANT, Some(OP_TYPE_IMAGE)) => {
                let im = ins.unwrap();
                match (im[3], im[7]) {
                    (DIM_SUBPASS, _) => "input_attachment",
                    (DIM_BUFFER, 2) => "storage_texel_buffer",
                    (DIM_BUFFER, _) => "uniform_texel_buffer",
                    (_, 2) => "storage_image",
                    _ => "sampled_image",
                }
            }
            _ => "unknown",
        };
        m.bindings.push(Binding {
            set: s,
            binding: b,
            kind,
            count,
            name: names.get(&id).cloned().unwrap_or_default(),
        });
    }
    m.bindings.sort();
    Ok(m)
}

impl Module {
    pub fn bindings_json(&self) -> Value {
        Value::Array(self.bindings.iter().map(|b| json!({"set": b.set, "binding": b.binding, "type": b.kind, "count": b.count, "name": b.name})).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ins(op: u32, args: &[u32]) -> Vec<u32> {
        let mut v = vec![((args.len() as u32 + 1) << 16) | op];
        v.extend_from_slice(args);
        v
    }

    #[test]
    fn bindings_from_types() {
        // uniform block at (0,0), sampled 2D image array of 4 at (5,1), storage buffer runtime array at (5,2)
        let mut w = vec![0x0723_0203, 0x0001_0300, 0, 100, 0];
        w.extend(ins(OP_CAPABILITY, &[1]));
        w.extend(ins(OP_ENTRY_POINT, &[4, 1, 0x6e69616d, 0]));
        w.extend(ins(OP_DECORATE, &[10, DEC_SET, 0]));
        w.extend(ins(OP_DECORATE, &[10, DEC_BINDING, 0]));
        w.extend(ins(OP_DECORATE, &[20, DEC_SET, 5]));
        w.extend(ins(OP_DECORATE, &[20, DEC_BINDING, 1]));
        w.extend(ins(OP_DECORATE, &[30, DEC_SET, 5]));
        w.extend(ins(OP_DECORATE, &[30, DEC_BINDING, 2]));
        w.extend(ins(OP_DECORATE, &[11, DEC_BLOCK]));
        w.extend(ins(21, &[2, 32, 0])); // uint
        w.extend(ins(OP_TYPE_STRUCT, &[11, 2]));
        w.extend(ins(OP_TYPE_POINTER, &[12, SC_UNIFORM, 11]));
        w.extend(ins(22, &[3, 32])); // float
        w.extend(ins(OP_TYPE_IMAGE, &[21, 3, 1, 0, 0, 0, 1, 0]));
        w.extend(ins(OP_TYPE_SAMPLED_IMAGE, &[22, 21]));
        w.extend(ins(OP_CONSTANT, &[2, 23, 4]));
        w.extend(ins(OP_TYPE_ARRAY, &[24, 22, 23]));
        w.extend(ins(OP_TYPE_POINTER, &[25, SC_UNIFORM_CONSTANT, 24]));
        w.extend(ins(OP_TYPE_STRUCT, &[31, 2]));
        w.extend(ins(OP_TYPE_RUNTIME_ARRAY, &[32, 31]));
        w.extend(ins(OP_TYPE_POINTER, &[33, SC_STORAGE_BUFFER, 32]));
        w.extend(ins(OP_VARIABLE, &[12, 10, SC_UNIFORM]));
        w.extend(ins(OP_VARIABLE, &[25, 20, SC_UNIFORM_CONSTANT]));
        w.extend(ins(OP_VARIABLE, &[33, 30, SC_STORAGE_BUFFER]));
        let b: Vec<u8> = w.iter().flat_map(|x| x.to_le_bytes()).collect();
        let m = reflect(&b).unwrap();
        assert_eq!(m.execution_model, Some("fragment"));
        let got: Vec<(u32, u32, &str, u32)> = m
            .bindings
            .iter()
            .map(|b| (b.set, b.binding, b.kind, b.count))
            .collect();
        assert_eq!(
            got,
            vec![
                (0, 0, "uniform_buffer", 1),
                (5, 1, "combined_image_sampler", 4),
                (5, 2, "storage_buffer", 0)
            ]
        );
    }
}
