//! Edits to the translator's SPIR-V that Vulkan needs, each small and named (the reflection lists the ones applied).
//!
//! - `storage_format_caps`: a storage image of unknown format that the module reads or writes needs the
//!   StorageImageReadWithoutFormat / StorageImageWriteWithoutFormat capability (Vulkan features
//!   shaderStorageImageReadWithoutFormat / shaderStorageImageWriteWithoutFormat). The translator does not declare
//!   them, and spirv-val refuses such a module.
//! - `size_query_components`: a texture size query whose result has fewer components than the shader reads (its
//!   depth or layer count, read from a texture the translator had to assume 2D for want of a per-game type answer)
//!   reads 1 for each missing component, the depth and layer count of a 2D texture. spirv-val refuses the
//!   out-of-range extract.
//! - `dynamic_offsets`: a fetch or read with a variable `Offset` (texelFetchOffset with a computed offset) gets the
//!   offset added to its coordinate; Vulkan allows non-constant offsets on gathers only. A constant one becomes
//!   `ConstOffset`.

use std::collections::HashSet;

/// The edits' version: part of each output's identity, so a change here translates every shader again.
pub const VERSION: &str = "fixes 3: storage_format_caps size_query_components dynamic_offsets";

const OP_CAPABILITY: u32 = 17;
const OP_TYPE_IMAGE: u32 = 25;
const OP_LOAD: u32 = 61;
const OP_IMAGE_READ: u32 = 98;
const OP_IMAGE_WRITE: u32 = 99;
const OP_IMAGE_SPARSE_READ: u32 = 320;
const CAP_READ_WITHOUT_FORMAT: u32 = 55;
const CAP_WRITE_WITHOUT_FORMAT: u32 = 56;

fn to_bytes(w: &[u32]) -> Vec<u8> {
    w.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// The capabilities a module's reads and writes of unknown-format storage images need and it lacks, added after its
/// other capabilities. → (the new module, names of the edits applied); None when nothing needs adding.
pub fn storage_format_caps(spv: &[u8]) -> Option<(Vec<u8>, Vec<&'static str>)> {
    let w = crate::reflect::words(spv)?;
    let mut caps = HashSet::new();
    let mut unknown_images = HashSet::new();
    let mut result_type = std::collections::HashMap::new(); // loaded value id -> its type id
    let mut reads = vec![];
    let mut writes = vec![];
    let mut last_cap = None;
    let mut i = 5;
    while i < w.len() {
        let n = (w[i] >> 16) as usize;
        if n == 0 || i + n > w.len() {
            return None;
        }
        match w[i] & 0xffff {
            OP_CAPABILITY => {
                caps.insert(w[i + 1]);
                last_cap = Some(i + n);
            }
            OP_TYPE_IMAGE if n >= 9 && w[i + 7] == 2 && w[i + 8] == 0 => {
                unknown_images.insert(w[i + 1]);
            }
            OP_LOAD if n >= 4 => {
                result_type.insert(w[i + 2], w[i + 1]);
            }
            OP_IMAGE_READ | OP_IMAGE_SPARSE_READ if n >= 5 => reads.push(w[i + 3]),
            OP_IMAGE_WRITE if n >= 4 => writes.push(w[i + 1]),
            _ => {}
        }
        i += n;
    }
    let unknown = |img: &u32| {
        result_type
            .get(img)
            .is_some_and(|t| unknown_images.contains(t))
    };
    let mut add = vec![];
    if !caps.contains(&CAP_READ_WITHOUT_FORMAT) && reads.iter().any(unknown) {
        add.push((CAP_READ_WITHOUT_FORMAT, "storage_image_read_without_format"));
    }
    if !caps.contains(&CAP_WRITE_WITHOUT_FORMAT) && writes.iter().any(unknown) {
        add.push((
            CAP_WRITE_WITHOUT_FORMAT,
            "storage_image_write_without_format",
        ));
    }
    let at = last_cap?;
    if add.is_empty() {
        return None;
    }
    let mut out = w[..at].to_vec();
    for (c, _) in &add {
        out.push((2 << 16) | OP_CAPABILITY);
        out.push(*c);
    }
    out.extend_from_slice(&w[at..]);
    Some((to_bytes(&out), add.into_iter().map(|a| a.1).collect()))
}

const OP_TYPE_INT: u32 = 21;
const OP_TYPE_VECTOR: u32 = 23;
const OP_CONSTANT: u32 = 43;
const OP_FUNCTION: u32 = 54;
const OP_COMPOSITE_EXTRACT: u32 = 81;
const OP_COPY_OBJECT: u32 = 83;
const OP_IMAGE_QUERY_SIZE_LOD: u32 = 103;
const OP_IMAGE_QUERY_SIZE: u32 = 104;

/// Out-of-range component reads of texture size queries become the constant 1 (see the module comment).
pub fn size_query_components(spv: &[u8]) -> Option<(Vec<u8>, Vec<&'static str>)> {
    let mut w = crate::reflect::words(spv)?;
    let mut vec_len = std::collections::HashMap::new(); // vector type -> (component type, count)
    let mut queries = std::collections::HashMap::new(); // query result -> component count
    let mut ones = std::collections::HashMap::new(); // int type -> its constant 1
    let mut ints = HashSet::new();
    let mut bad = vec![]; // (instruction index, result type)
    let mut first_fn = None;
    let mut i = 5;
    while i < w.len() {
        let n = (w[i] >> 16) as usize;
        if n == 0 || i + n > w.len() {
            return None;
        }
        match w[i] & 0xffff {
            OP_TYPE_INT => {
                ints.insert(w[i + 1]);
            }
            OP_TYPE_VECTOR if n >= 4 => {
                vec_len.insert(w[i + 1], (w[i + 2], w[i + 3]));
            }
            OP_CONSTANT if n == 4 && w[i + 3] == 1 && ints.contains(&w[i + 1]) => {
                ones.entry(w[i + 1]).or_insert(w[i + 2]);
            }
            OP_FUNCTION if first_fn.is_none() => first_fn = Some(i),
            OP_IMAGE_QUERY_SIZE_LOD | OP_IMAGE_QUERY_SIZE if n >= 4 => {
                if let Some(&(_, c)) = vec_len.get(&w[i + 1]) {
                    queries.insert(w[i + 2], c);
                }
            }
            OP_COMPOSITE_EXTRACT if n == 5 => {
                if let Some(&c) = queries.get(&w[i + 3]) {
                    if w[i + 4] >= c && ints.contains(&w[i + 1]) {
                        bad.push((i, w[i + 1]));
                    }
                }
            }
            _ => {}
        }
        i += n;
    }
    if bad.is_empty() {
        return None;
    }
    let mut new_consts = vec![];
    for &(_, t) in &bad {
        if !ones.contains_key(&t) {
            let id = w[3];
            w[3] += 1;
            ones.insert(t, id);
            new_consts.extend([(4 << 16) | OP_CONSTANT, t, id, 1]);
        }
    }
    for &(at, t) in &bad {
        // OpCompositeExtract type result composite index (5 words) -> OpCopyObject type result one + OpNop-free pad:
        // rewritten in place as a 4-word OpCopyObject followed by a 1-word OpNop
        let res = w[at + 2];
        w[at] = (4 << 16) | OP_COPY_OBJECT;
        w[at + 1] = t;
        w[at + 2] = res;
        w[at + 3] = ones[&t];
        w[at + 4] = 1 << 16; // OpNop
    }
    let at = first_fn?;
    let mut out = w[..at].to_vec();
    out.extend(new_consts);
    out.extend_from_slice(&w[at..]);
    Some((to_bytes(&out), vec!["size_query_components"]))
}

const OP_IMAGE_FETCH: u32 = 95;
const OP_COMPOSITE_CONSTRUCT: u32 = 80;
const OP_IADD: u32 = 128;
const IMG_OFFSET: u32 = 0x10;

/// The words each image operand bit carries after the mask, in bit order (SPIR-V 3.14 Image Operands).
fn image_operand_words(bit: u32) -> usize {
    match bit {
        0x4 => 2, // Grad
        0x1 | 0x2 | 0x8 | 0x10 | 0x20 | 0x40 | 0x80 | 0x100 | 0x200 | 0x10000 => 1,
        _ => 0,
    }
}

/// Opcodes whose word 1 is a result type and word 2 a result (the value instructions coordinates and offsets come
/// from: constants, loads, access chains, composites, conversions, arithmetic, image instructions, extended ones).
fn has_type_and_result(op: u32) -> bool {
    matches!(op, 12 | 41..=46 | 50..=52 | 59 | 61 | 65..=70 | 77..=84 | 86..=107 | 109..=202 | 227..=242 | 300..=320 | 4421..=4500)
}

/// A dynamic `Offset` on an integer-coordinate fetch or read (texelFetchOffset with a variable offset): Vulkan allows a
/// non-constant offset only on gathers (VUID-StandaloneSpirv-Offset-04663), so the offset is added to the coordinate
/// (`OpIAdd`, widened with zeros when the coordinate also holds a layer) and the operand dropped. A constant one becomes
/// `ConstOffset`. Ocarina of Time's d17eefaff4f0bc23 (compute) and Pokémon Violet's abd67260d710b0a0 (pixel) have them.
pub fn dynamic_offsets(spv: &[u8]) -> Option<(Vec<u8>, Vec<&'static str>)> {
    let w = crate::reflect::words(spv)?;
    let mut types: std::collections::HashMap<u32, u32> = std::collections::HashMap::new(); // value -> type
    let mut vecs: std::collections::HashMap<u32, (u32, u32)> = std::collections::HashMap::new(); // vector type -> (component, count)
    let mut ints: HashSet<u32> = HashSet::new();
    let mut consts: HashSet<u32> = HashSet::new();
    let mut zeros: std::collections::HashMap<u32, u32> = std::collections::HashMap::new(); // int type -> its constant 0
    let mut first_fn = None;
    let mut targets = vec![];
    let mut i = 5;
    while i < w.len() {
        let n = (w[i] >> 16) as usize;
        if n == 0 || i + n > w.len() {
            return None;
        }
        let op = w[i] & 0xffff;
        match op {
            OP_TYPE_INT => {
                ints.insert(w[i + 1]);
            }
            OP_TYPE_VECTOR if n >= 4 => {
                vecs.insert(w[i + 1], (w[i + 2], w[i + 3]));
            }
            OP_FUNCTION if first_fn.is_none() => first_fn = Some(i),
            _ => {}
        }
        if (41..=46).contains(&op) || op == OP_CONSTANT {
            consts.insert(w[i + 2]);
        }
        if op == OP_CONSTANT && n == 4 && w[i + 3] == 0 && ints.contains(&w[i + 1]) {
            zeros.entry(w[i + 1]).or_insert(w[i + 2]);
        }
        if has_type_and_result(op) && n >= 3 {
            types.insert(w[i + 2], w[i + 1]);
        }
        if (op == OP_IMAGE_FETCH || op == OP_IMAGE_READ) && n >= 6 && w[i + 5] & IMG_OFFSET != 0 {
            targets.push(i);
        }
        i += n;
    }
    if targets.is_empty() {
        return None;
    }
    let mut bound = w[3];
    let mut new_consts: Vec<u32> = vec![];
    let mut edits: std::collections::HashMap<usize, Vec<u32>> = std::collections::HashMap::new(); // index -> replacement words
    let mut names = vec![];
    for &at in &targets {
        let n = (w[at] >> 16) as usize;
        let op = w[at] & 0xffff;
        let (rt, res, img, coord, mask) = (w[at + 1], w[at + 2], w[at + 3], w[at + 4], w[at + 5]);
        // the operands after the mask, by bit
        let mut ops: Vec<(u32, Vec<u32>)> = vec![];
        let mut k = at + 6;
        let mut bit = 1u32;
        while bit != 0 && bit <= 0x10000 {
            if mask & bit != 0 {
                let c = image_operand_words(bit);
                ops.push((bit, w[k..k + c].to_vec()));
                k += c;
            }
            bit <<= 1;
        }
        if k != at + n {
            return None;
        }
        let off = ops.iter().find(|o| o.0 == IMG_OFFSET)?.1[0];
        if consts.contains(&off) {
            // a constant offset: ConstOffset (bit 0x8, ordered before Offset)
            let mut out = vec![0u32, rt, res, img, coord, (mask & !IMG_OFFSET) | 0x8];
            let mut sorted: Vec<(u32, Vec<u32>)> = ops
                .iter()
                .map(|(b, v)| (if *b == IMG_OFFSET { 0x8 } else { *b }, v.clone()))
                .collect();
            sorted.sort_by_key(|o| o.0);
            for (_, v) in sorted {
                out.extend(v);
            }
            out[0] = ((out.len() as u32) << 16) | op;
            edits.insert(at, out);
            names.push("const_offset");
            continue;
        }
        let (Some(&ct), Some(&ot)) = (types.get(&coord), types.get(&off)) else {
            return None;
        };
        let mut pre: Vec<u32> = vec![];
        let add_to = if ct == ot {
            off
        } else {
            // widen the offset to the coordinate's components with zeros (an array layer, a 3D coordinate's z)
            let (&(cc, cn), &(oc, on)) = (vecs.get(&ct)?, vecs.get(&ot).unwrap_or(&(ot, 1)));
            if !ints.contains(&cc) || on >= cn {
                return None;
            }
            let zero = match zeros.get(&cc) {
                Some(z) => *z,
                None => {
                    let z = bound;
                    bound += 1;
                    new_consts.extend([(4 << 16) | OP_CONSTANT, cc, z, 0]);
                    zeros.insert(cc, z);
                    z
                }
            };
            let mut parts = vec![];
            for c in 0..on {
                if vecs.contains_key(&ot) {
                    let id = bound;
                    bound += 1;
                    pre.extend([(5 << 16) | OP_COMPOSITE_EXTRACT, oc, id, off, c]);
                    parts.push(id);
                } else {
                    parts.push(off);
                }
            }
            while (parts.len() as u32) < cn {
                parts.push(zero);
            }
            let id = bound;
            bound += 1;
            pre.push(((3 + parts.len() as u32) << 16) | OP_COMPOSITE_CONSTRUCT);
            pre.extend([ct, id]);
            pre.extend(&parts);
            id
        };
        let sum = bound;
        bound += 1;
        pre.extend([(5 << 16) | OP_IADD, ct, sum, coord, add_to]);
        let rest_mask = mask & !IMG_OFFSET;
        let mut out = pre;
        let mut inst = vec![0u32, rt, res, img, sum];
        if rest_mask != 0 {
            inst.push(rest_mask);
            for (b, v) in &ops {
                if *b != IMG_OFFSET {
                    inst.extend(v);
                }
            }
        }
        inst[0] = ((inst.len() as u32) << 16) | op;
        out.extend(inst);
        edits.insert(at, out);
        names.push("dynamic_offset_added");
    }
    let mut out = w[..5].to_vec();
    out[3] = bound;
    let mut i = 5;
    while i < w.len() {
        let n = (w[i] >> 16) as usize;
        if Some(i) == first_fn {
            out.extend(&new_consts);
        }
        match edits.get(&i) {
            Some(e) => out.extend(e),
            None => out.extend_from_slice(&w[i..i + n]),
        }
        i += n;
    }
    names.sort();
    names.dedup();
    Some((to_bytes(&out), names))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_write_without_format() {
        let mut w = vec![0x0723_0203u32, 0x0001_0300, 0, 50, 0];
        w.extend([(2 << 16) | OP_CAPABILITY, 1]);
        w.extend([(9 << 16) | OP_TYPE_IMAGE, 10, 3, 1, 0, 0, 0, 2, 0]); // 2D storage image, unknown format
        w.extend([(4 << 16) | OP_LOAD, 10, 11, 12]);
        w.extend([(4 << 16) | OP_IMAGE_WRITE, 11, 13, 14]);
        let b = to_bytes(&w);
        let (out, names) = storage_format_caps(&b).unwrap();
        assert_eq!(names, vec!["storage_image_write_without_format"]);
        let ow = crate::reflect::words(&out).unwrap();
        assert_eq!(
            &ow[7..9],
            &[(2 << 16) | OP_CAPABILITY, CAP_WRITE_WITHOUT_FORMAT]
        );
        assert!(storage_format_caps(&out).is_none());
    }

    #[test]
    fn dynamic_fetch_offset_is_added_to_the_coordinate() {
        let mut w = vec![0x0723_0203u32, 0x0001_0300, 0, 100, 0];
        w.extend([(2 << 16) | OP_CAPABILITY, 1]);
        w.extend([(4 << 16) | OP_TYPE_INT, 20, 32, 1]);
        w.extend([(4 << 16) | OP_TYPE_VECTOR, 21, 20, 2]);
        w.extend([(4 << 16) | OP_CONSTANT, 20, 22, 0]);
        w.extend([(5 << 16) | OP_FUNCTION, 1, 2, 0, 3]);
        w.extend([(5 << 16) | OP_COMPOSITE_CONSTRUCT, 21, 30, 40, 41]); // coordinate
        w.extend([(5 << 16) | OP_COMPOSITE_CONSTRUCT, 21, 31, 42, 43]); // offset (computed)
        w.extend([
            (8 << 16) | OP_IMAGE_FETCH,
            50,
            51,
            52,
            30,
            0x2 | 0x10,
            22,
            31,
        ]);
        let (out, names) = dynamic_offsets(&to_bytes(&w)).unwrap();
        assert_eq!(names, vec!["dynamic_offset_added"]);
        let o = crate::reflect::words(&out).unwrap();
        assert_eq!(o[3], 101);
        assert!(o
            .windows(5)
            .any(|x| x == [(5 << 16) | OP_IADD, 21, 100, 30, 31]));
        assert!(o
            .windows(7)
            .any(|x| x == [(7 << 16) | OP_IMAGE_FETCH, 50, 51, 52, 100, 0x2, 22]));
    }

    #[test]
    fn size_query_component_out_of_range() {
        let mut w = vec![0x0723_0203u32, 0x0001_0300, 0, 60, 0];
        w.extend([(2 << 16) | OP_CAPABILITY, 1]);
        w.extend([(4 << 16) | OP_TYPE_INT, 20, 32, 1]);
        w.extend([(4 << 16) | OP_TYPE_VECTOR, 21, 20, 2]);
        w.extend([(5 << 16) | OP_FUNCTION, 1, 2, 0, 3]);
        w.extend([(5 << 16) | OP_IMAGE_QUERY_SIZE_LOD, 21, 30, 31, 32]);
        w.extend([(5 << 16) | OP_COMPOSITE_EXTRACT, 20, 33, 30, 1]);
        w.extend([(5 << 16) | OP_COMPOSITE_EXTRACT, 20, 34, 30, 2]);
        let (out, names) = size_query_components(&to_bytes(&w)).unwrap();
        assert_eq!(names, vec!["size_query_components"]);
        let o = crate::reflect::words(&out).unwrap();
        assert_eq!(o[3], 61);
        let at = o
            .iter()
            .position(|&x| x == (5 << 16) | OP_FUNCTION)
            .unwrap();
        assert_eq!(&o[at - 4..at], &[(4 << 16) | OP_CONSTANT, 20, 60, 1]);
        assert!(o
            .windows(4)
            .any(|x| x == [(4 << 16) | OP_COPY_OBJECT, 20, 34, 60]));
        assert!(o
            .windows(5)
            .any(|x| x == [(5 << 16) | OP_COMPOSITE_EXTRACT, 20, 33, 30, 1]));
    }
}
