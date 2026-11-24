//! Native mesh admission checks. Unknown stage-memory layouts are refused, not guessed.
use std::collections::HashMap;

/// Host record layout only, shared by the actual GPU copy and DrawIndex binding.
/// Does not read or validate GPU-generated group counts. Preserves the held baseline.
pub struct IndirectLayout {
    pub count: u32,
    pub stride: u64,
    pub span: u64,
    pub count_bytes: u64,
    pub shader_stride: u32,
}
pub fn indirect_layout(count: i32, stride: i32, three_d: bool) -> IndirectLayout {
    let count = count.max(1) as u32;
    let record = if three_d { 12 } else { 8 };
    let stride = if stride > 0 { stride as u64 } else { record };
    IndirectLayout { count, stride, span: stride * u64::from(count - 1) + record,
        count_bytes: if three_d { 12 } else { 4 },
        shader_stride: if three_d { 0 } else { (stride / 4) as u32 } }
}

#[derive(Clone, Copy, Debug)]
pub struct GridLimits { pub axes: [u32; 3], pub total: u32 }
pub fn grid(groups: [u32; 3], limits: GridLimits) -> Result<(), &'static str> {
    if groups.iter().zip(limits.axes).any(|(&n, max)| n > max) {
        return Err("mesh grid axis exceeds device limit");
    }
    let count = groups.into_iter().try_fold(1u32, |a, b| a.checked_mul(b))
        .ok_or("mesh grid product overflow")?;
    if count > limits.total { return Err("mesh grid total exceeds device limit"); }
    Ok(())
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stage {
    pub local: [u32; 3],
    pub shared: u32,
    pub payload: u32,
    pub vertices: u32,
    pub primitives: u32,
    /// Effective scalar components: four per occupied output location.
    pub output_components: [u32; 2],
}

/// The translator emits scalar/vector fixed arrays for stage memory and payload.
/// Struct padding, specialization constants and runtime-sized arrays require a
/// different layout proof; they are explicitly outside this admission contract.
pub fn reflect(words: &[u32]) -> Result<Stage, &'static str> {
    if words.len() < 5 || words[0] != 0x07230203 { return Err("invalid mesh SPIR-V"); }
    let mut types: HashMap<u32, (u32, Vec<u32>)> = HashMap::new();
    let mut constants = HashMap::new();
    let mut variables = Vec::new();
    let mut outputs = Vec::new();
    let mut builtins = HashMap::new();
    let mut locations = HashMap::new();
    let mut per_primitive = std::collections::HashSet::new();
    let mut strides = std::collections::HashSet::new();
    let mut result = Stage::default();
    let mut at = 5;
    while at < words.len() {
        let n = (words[at] >> 16) as usize;
        if n == 0 || n > words.len() - at { return Err("truncated mesh instruction"); }
        let op = words[at] & 0xffff;
        let a = &words[at + 1..at + n];
        match (op, a) {
            (16, [_, 17, x, y, z]) => result.local = [*x, *y, *z],
            (16, [_, 26, v]) => result.vertices = *v,
            (16, [_, 5270, p]) => result.primitives = *p,
            (21 | 22 | 23 | 28 | 32, [id, rest @ ..]) => { types.insert(*id, (op, rest.to_vec())); }
            (43, [_, id, value]) => { constants.insert(*id, *value); }
            (59, [ty, _, storage, ..]) if matches!(*storage, 4 | 5402) => variables.push((*ty, *storage)),
            (59, [ty, id, 3, ..]) => outputs.push((*ty, *id)),
            (71, [id, 11, builtin]) => { builtins.insert(*id, *builtin); }
            (71, [id, 30, location]) => { locations.insert(*id, *location); }
            (71, [id, 5271]) => { per_primitive.insert(*id); }
            (71, [id, 6, _]) => { strides.insert(*id); }
            _ => {}
        }
        at += n;
    }
    fn bytes(id: u32, types: &HashMap<u32, (u32, Vec<u32>)>, constants: &HashMap<u32, u32>, depth: u32)
        -> Option<u32> {
        if depth > 16 { return None; }
        let (op, a) = types.get(&id)?;
        match (*op, a.as_slice()) {
            (21, [32, _]) | (22, [32]) => Some(4),
            (23, [element, count]) => bytes(*element, types, constants, depth + 1)?.checked_mul(*count),
            (28, [element, count]) => bytes(*element, types, constants, depth + 1)?.checked_mul(*constants.get(count)?),
            (32, [4 | 5402, element]) => bytes(*element, types, constants, depth + 1),
            _ => None,
        }
    }
    fn plain_storage(id: u32, types: &HashMap<u32, (u32, Vec<u32>)>,
        strides: &std::collections::HashSet<u32>, depth: u32) -> bool {
        if depth > 16 || strides.contains(&id) { return false; }
        match types.get(&id).map(|(op, a)| (*op, a.as_slice())) {
            Some((21, [32, _]) | (22, [32])) => true,
            Some((28, [element, _]) | (32, [4 | 5402, element])) => plain_storage(*element, types, strides, depth + 1),
            _ => false,
        }
    }
    for (ty, storage) in variables {
        if !plain_storage(ty, &types, &strides, 0) { return Err("unknown mesh stage-memory padding"); }
        let size = bytes(ty, &types, &constants, 0).ok_or("unknown mesh stage-memory layout")?;
        let target = if storage == 4 { &mut result.shared } else { &mut result.payload };
        *target = target.checked_add(size).ok_or("mesh stage-memory overflow")?;
    }
    // Interface arrays consume locations per element, unlike packed stage memory.
    // In particular float[16] uses sixteen locations, not ceil(64 bytes / 16).
    fn interface_locations(id: u32, types: &HashMap<u32, (u32, Vec<u32>)>,
        constants: &HashMap<u32, u32>, depth: u32) -> Option<u32> {
        if depth > 16 { return None; }
        let (op, a) = types.get(&id)?;
        match (*op, a.as_slice()) {
            (21, [32, _]) | (22, [32]) => Some(1),
            (23, [element, lanes]) if (2..=4).contains(lanes) => {
                match types.get(element).map(|(op, a)| (*op, a.as_slice())) {
                    Some((21, [32, _]) | (22, [32])) => Some(1),
                    _ => None,
                }
            }
            (28, [element, length]) => {
                let count = *constants.get(length)?;
                if count == 0 { return None; }
                interface_locations(*element, types, constants, depth + 1)?.checked_mul(count)
            }
            _ => None,
        }
    }
    if builtins.values().any(|&b| b == 4440) { return Err("multiview mesh output needs a view-count contract"); }
    let mut occupied: [Vec<(u32, u32)>; 2] = Default::default();
    for (ty, id) in outputs {
        if builtins.get(&id).is_some_and(|b| (5294..=5296).contains(b)) { continue; }
        let (32, pointer) = types.get(&ty).ok_or("unknown mesh output pointer")? else {
            return Err("unknown mesh output pointer");
        };
        let array = *pointer.get(1).ok_or("short mesh output pointer")?;
        let (28, array) = types.get(&array).ok_or("unknown mesh output array")? else {
            return Err("mesh output is not a fixed array");
        };
        let element = *array.first().ok_or("short mesh output array")?;
        let slots = interface_locations(element, &types, &constants, 0)
            .ok_or("unknown or overflowing mesh interface layout")?;
        let kind = usize::from(per_primitive.contains(&id));
        if let Some(&location) = locations.get(&id) {
            occupied[kind].push((location, location.checked_add(slots).ok_or("mesh output location overflow")?));
        } else if builtins.contains_key(&id) {
            result.output_components[kind] = result.output_components[kind].checked_add(
                slots.checked_mul(4).ok_or("mesh output component overflow")?).ok_or("mesh output component overflow")?;
        } else { return Err("mesh output has no location or builtin"); }
    }
    for (kind, ranges) in occupied.iter_mut().enumerate() {
        // Interval union keeps work bounded by declarations, not declared array length.
        ranges.sort_unstable();
        let (mut end, mut slots) = (0, 0u32);
        for &(lo, hi) in ranges.iter() {
            slots = slots.checked_add(hi.saturating_sub(end.max(lo)))
                .ok_or("mesh output location overflow")?;
            end = end.max(hi);
        }
        result.output_components[kind] = result.output_components[kind].checked_add(
            slots.checked_mul(4).ok_or("mesh output component overflow")?).ok_or("mesh output component overflow")?;
    }
    if result.local.contains(&0) { return Err("unknown mesh local size"); }
    Ok(result)
}

#[derive(Clone, Copy)]
pub struct Limits {
    pub local: GridLimits,
    pub shared: u32,
    pub payload: u32,
    pub combined: u32,
    pub vertices: u32,
    pub primitives: u32,
    /// Output bytes, payload+output bytes, components, vertex/primitive allocation granularity.
    pub output: [u32; 5],
}
pub fn validate(stage: &Stage, limits: Limits) -> Result<(), &'static str> {
    grid(stage.local, limits.local)?;
    let combined = stage.shared.checked_add(stage.payload).ok_or("mesh combined memory overflow")?;
    if stage.shared > limits.shared || stage.payload > limits.payload || combined > limits.combined {
        return Err("mesh payload/shared memory exceeds device limit");
    }
    if stage.vertices > limits.vertices || stage.primitives > limits.primitives {
        return Err("mesh output counts exceed device limit");
    }
    if stage.output_components != [0; 2] {
        let components = stage.output_components[0].checked_add(stage.output_components[1])
            .ok_or("mesh output component overflow")?;
        let mut size = 0u32;
        for (i, count) in [stage.vertices, stage.primitives].into_iter().enumerate() {
            let granularity = limits.output[3 + i];
            if granularity == 0 { return Err("unknown mesh output granularity"); }
            let aligned = count.div_ceil(granularity).checked_mul(granularity)
                .ok_or("mesh output allocation overflow")?;
            size = size.checked_add(stage.output_components[i].checked_mul(4)
                .and_then(|n| n.checked_mul(aligned)).ok_or("mesh output allocation overflow")?)
                .ok_or("mesh output allocation overflow")?;
        }
        if components > limits.output[2] || size > limits.output[0]
            || size.checked_add(stage.payload).is_none_or(|n| n > limits.output[1]) {
            return Err("mesh output/payload memory or components exceed device limit");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn indirect_gpu_copy_layout_preserves_first_and_draw_index_stride() {
        let one = indirect_layout(1, 0, false);
        assert_eq!((one.count, one.span, one.count_bytes, one.shader_stride), (1, 8, 4, 2));
        // A count-buffer draw uses n as its maximum, with the identical record plan.
        let counted = indirect_layout(3, 16, false);
        assert_eq!((counted.span, counted.count_bytes, counted.shader_stride), (40, 4, 4));
        // first is word DrawIndex*stride+1 in the separately copied 40-byte span.
        assert_eq!((2 * counted.shader_stride + 1) * 4, 36);
        let three = indirect_layout(3, 16, true);
        assert_eq!((three.span, three.count_bytes, three.shader_stride), (44, 12, 0));
        assert_eq!(indirect_layout(1, 0, true).stride, 12);
    }
    #[test]
    fn taskless_axes_total_and_overflow_are_independent() {
        let task = GridLimits { axes: [100; 3], total: 1000 };
        let mesh = GridLimits { axes: [10; 3], total: 64 };
        assert!(grid([8, 8, 1], mesh).is_ok());
        assert!(grid([8, 8, 2], mesh).is_err());
        assert!(grid([11, 1, 1], task).is_ok());
        assert!(grid([11, 1, 1], mesh).is_err());
        assert!(grid([u32::MAX, 2, 1], GridLimits { axes: [u32::MAX; 3], total: u32::MAX }).is_err());
    }
    #[test]
    fn actual_spirv_fixed_payload_and_stage_memory_are_measured() {
        let mut w = vec![0x07230203, 0x10400, 0, 20, 0];
        for (op, a) in [(16, vec![1, 17, 32, 1, 1]), (21, vec![2, 32, 0]),
            (43, vec![2, 3, 1024]), (28, vec![4, 2, 3]), (32, vec![5, 5402, 4]),
            (59, vec![5, 6, 5402]), (32, vec![7, 4, 4]), (59, vec![7, 8, 4])] {
            w.push(((a.len() as u32 + 1) << 16) | op); w.extend(a);
        }
        let stage = reflect(&w).unwrap();
        assert_eq!((stage.payload, stage.shared), (4096, 4096));
        let limits = Limits { local: GridLimits { axes: [32, 1, 1], total: 32 },
            shared: 4096, payload: 4096, combined: 8192, vertices: 256, primitives: 256,
            output: [4096, 8192, 128, 32, 32] };
        assert!(validate(&stage, limits).is_ok());
        assert!(validate(&stage, Limits { combined: 8191, ..limits }).is_err());
        let output = Stage { vertices: 33, primitives: 1, output_components: [4, 4], ..stage };
        assert!(validate(&output, Limits { output: [1536, 5632, 8, 32, 32], ..limits }).is_ok());
        assert!(validate(&output, Limits { output: [1535, 5632, 8, 32, 32], ..limits }).is_err());
        w.pop();
        assert!(reflect(&w).is_err());
    }

    fn array_output_fixture(vec2: bool, lengths: &[u32]) -> Vec<u32> {
        let mut w = vec![0x07230203, 0x10400, 0, 128, 0];
        let mut emit = |op: u32, a: &[u32]| {
            w.push(((a.len() as u32 + 1) << 16) | op); w.extend_from_slice(a);
        };
        emit(16, &[1, 17, 32, 1, 1]);
        emit(16, &[1, 26, 256]);
        emit(16, &[1, 5270, 1]);
        emit(21, &[2, 32, 0]);
        emit(22, &[3, 32]);
        emit(23, &[4, 3, 4]); // Position
        emit(23, &[5, 3, 2]);
        emit(43, &[2, 6, 256]);
        let mut element = if vec2 { 5 } else { 3 };
        for (i, &length) in lengths.iter().enumerate() {
            let id = 10 + 2 * i as u32;
            emit(43, &[2, id, length]);
            emit(28, &[id + 1, element, id]);
            element = id + 1;
        }
        emit(28, &[50, element, 6]); // strip only this per-vertex dimension
        emit(32, &[51, 3, 50]);
        emit(59, &[51, 52, 3]);
        emit(71, &[52, 30, 0]);
        emit(28, &[60, 4, 6]);
        emit(32, &[61, 3, 60]);
        emit(59, &[61, 62, 3]);
        emit(71, &[62, 11, 0]); // Position builtin
        w
    }

    #[test]
    fn nested_interface_arrays_use_locations_not_packed_bytes() {
        let limits = Limits { local: GridLimits { axes: [32, 1, 1], total: 32 },
            shared: 32768, payload: 4096, combined: 32768, vertices: 256, primitives: 256,
            output: [32768, 100000, 128, 32, 32] };
        let scalar = reflect(&array_output_fixture(false, &[16])).unwrap();
        assert_eq!(scalar.output_components, [68, 0]);
        assert!(validate(&scalar, limits).is_err()); // actual requirement: 69632, not 20480
        assert!(validate(&scalar, Limits { output: [69632, 69632, 68, 32, 32], ..limits }).is_ok());
        assert!(validate(&scalar, Limits { output: [69631, 69632, 68, 32, 32], ..limits }).is_err());
        let nested = reflect(&array_output_fixture(true, &[3, 2])).unwrap();
        assert_eq!(nested.output_components, [28, 0]); // 2*3 locations plus Position
        assert!(validate(&nested, Limits { output: [28672, 28672, 28, 32, 32], ..limits }).is_ok());
        assert!(validate(&nested, Limits { output: [28671, 28672, 28, 32, 32], ..limits }).is_err());
        assert!(reflect(&array_output_fixture(false, &[u32::MAX, 2])).is_err());
    }
}
