//! CPU-side contracts shared by native preparation and its failure-injection tests.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufferSpan {
    pub buffer: u64,
    pub offset: u64,
    pub range: u64,
}

pub fn validate_buffers(
    buffers: [BufferSpan; 5],
    extents: [u64; 5],
    alignment: u64,
    max_range: u64,
    groups: u32,
    max_groups: u32,
) -> Result<(), &'static str> {
    if alignment == 0 || groups == 0 || groups > max_groups {
        return Err("NDVE device dispatch/alignment contract");
    }
    for (b, required) in buffers.iter().zip(extents) {
        if b.buffer == 0 || b.range < required || b.range > max_range
            || b.offset % alignment != 0 || b.offset.checked_add(b.range).is_none()
        {
            return Err("NDVE missing/short/unaligned/overflowing buffer");
        }
    }
    let out = buffers[1];
    for i in [0, 2, 3, 4] {
        let b = buffers[i];
        if b.buffer == out.buffer && b.offset < out.offset + out.range
            && out.offset < b.offset + b.range
        {
            return Err("NDVE input/output alias");
        }
    }
    Ok(())
}

/// Resolve semantic NVN register roles from reflection's driver-bank handle words.
/// Vulkan descriptor binding numbers are deliberately not used. Duplicate roles are
/// retained and must agree, rather than silently picking the first descriptor.
pub fn operand<T: Copy + Eq>(
    entries: impl IntoIterator<Item = (i32, u32, T)>,
    image: bool,
    register: u32,
) -> Result<T, &'static str> {
    let word = (if image { 0x120 } else { 0x20 }) / 4 + 2 * register;
    let mut found = None;
    for (bank, handle, value) in entries {
        let bank = if bank < 0 { bank } else { bank & 0xffff };
        if !matches!(bank, -1 | 2) || handle & 0x3fff != word { continue; }
        if found.is_some_and(|old| old != value) {
            return Err("ambiguous NDVE reflected register");
        }
        found = Some(value);
    }
    found.ok_or("missing NDVE reflected register")
}

/// All fallible preparation finishes before the caller receives anything recordable.
/// Owners returned by `prepare` drop normally on a partial failure; production native
/// descriptors instead register with the submission before returning their handles.
pub fn prepare_all<I, T, E>(
    items: impl IntoIterator<Item = I>,
    mut prepare: impl FnMut(I) -> Result<T, E>,
) -> Result<Vec<T>, E> {
    items.into_iter().map(&mut prepare).collect()
}

/// Consume only at replay, in captured order. Preparation never receives this queue.
pub fn replay_in_order<T>(pending: Vec<T>, mut replay: impl FnMut(&T)) {
    for snapshot in &pending { replay(snapshot); }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn roles_survive_decoys_remapping_and_duplicate_reflection() {
        let entries = [(2, 8, 100), (2, 10, 200), (0x30002, 10, 200)];
        assert_eq!(operand(entries, false, 1), Ok(200));
        assert!(operand([(2, 10, 200), (2, 10, 300)], false, 1).is_err());
        assert!(operand([(3, 10, 200)], false, 1).is_err());
        assert_eq!(operand([(2, 74, 400)], true, 1), Ok(400));
    }

    #[test]
    fn buffer_bounds_aliases_and_device_limits_are_checked_before_preparation() {
        let b = std::array::from_fn(|i| BufferSpan { buffer: i as u64 + 1, offset: 256, range: 512 });
        let check = |b, groups| validate_buffers(b, [512; 5], 256, 1024, groups, 65535);
        assert!(check(b, 65535).is_ok());
        assert!(check(b, 65536).is_err());
        for bad in [BufferSpan { offset: 1, ..b[0] }, BufferSpan { range: 511, ..b[0] },
            BufferSpan { range: 1025, ..b[0] }, BufferSpan { offset: u64::MAX - 255, ..b[0] }, b[1]] {
            let mut input = b;
            input[0] = bad;
            assert!(check(input, 1).is_err());
        }
    }

    #[test]
    fn every_partial_preparation_releases_exactly_its_constructed_owners() {
        struct Owner<'a>(usize, &'a RefCell<Vec<usize>>);
        impl Drop for Owner<'_> { fn drop(&mut self) { self.1.borrow_mut().push(self.0); } }
        for failure in 0..18 {
            let dropped = RefCell::new(Vec::new());
            let result = prepare_all(0..18, |i| {
                if i == failure { Err(i) } else { Ok(Owner(i, &dropped)) }
            });
            assert!(result.is_err());
            let mut actual = dropped.borrow().clone();
            actual.sort();
            assert_eq!(actual, (0..failure).collect::<Vec<_>>());
            let guest = vec![9, 2, 7, 4];
            let mut issued = Vec::new();
            replay_in_order(guest, |snapshot| issued.push(*snapshot));
            assert_eq!(issued, [9, 2, 7, 4]);
        }
    }
}
