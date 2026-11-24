//! Bounded CPU evidence for mesh descriptor-buffer draws. No GPU reads or Vulkan initialization.
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};

pub const MAX_RANGES: usize = 64;
const MAX_DRAWS: usize = 16_384;
const MAX_BYTES: usize = 16 << 20;

pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("NXGFX_MESH_BINDINGS").as_deref() == Ok("1"))
}

#[derive(Clone, Debug)]
pub struct Range {
    pub binding: u32,
    pub kind: i32,
    pub buffer: u64,
    pub offset: u64,
    /// The address/range actually passed to vkGetDescriptorEXT, including fallback.
    pub address: u64,
    pub size: u64,
    pub binding_id: Option<u64>,
    pub owned: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum Relation {
    Contains,
    Outside(u64),
    Unknown,
}

impl Range {
    fn relation(&self, address: u64) -> Relation {
        if self.address == 0 || self.size == 0 {
            return Relation::Unknown;
        }
        let Some(end) = self.address.checked_add(self.size) else {
            return Relation::Unknown;
        };
        if address < self.address {
            Relation::Outside(self.address - address)
        } else if address >= end {
            Relation::Outside(address - end)
        } else {
            Relation::Contains
        }
    }
}

#[derive(Debug)]
pub struct Stage {
    pub serial: u64,
    pub chunk: usize,
    pub offset: u64,
    pub stage: u32,
    pub shader: [u8; 8],
    /// Global pool epoch, not an individual allocation generation.
    pub pool_epoch: u64,
    pub ranges: Vec<Range>,
    pub omitted: usize,
}

impl Stage {
    pub fn push(&mut self, range: Range) {
        if self.ranges.len() < MAX_RANGES {
            self.ranges.push(range);
        } else {
            self.omitted += 1;
        }
    }

    pub fn matches(
        &self,
        serial: u64,
        chunk: usize,
        offset: u64,
        stage: u32,
        shader: [u8; 8],
    ) -> bool {
        (
            self.serial,
            self.chunk,
            self.offset,
            self.stage,
            self.shader,
        ) == (serial, chunk, offset, stage, shader)
    }
}

pub type StageUses = Vec<(Arc<Stage>, Vec<Option<u64>>)>;

pub struct Draw {
    pub checkpoint: u64,
    /// Last checkpoint recorded on the issuing thread before the mesh command. This recording span
    /// includes setup in pre/main buffers; it is a conservative association, not GPU execution order.
    pub setup_last: u64,
    pub serial: u64,
    pub command_buffer: u64,
    pub frame: u64,
    pub pool_epoch: u64,
    pub missing_stages: usize,
    pub stages: StageUses,
}

impl Draw {
    fn associates(&self, marker: u64) -> bool {
        self.checkpoint != 0 && marker >= self.checkpoint && marker <= self.setup_last
    }

    fn charge(&self) -> usize {
        // Conservatively charge shared snapshots for each draw too. No retained payload grows unbounded.
        std::mem::size_of::<Self>()
            + self.stages.capacity() * std::mem::size_of::<(Arc<Stage>, Vec<Option<u64>>)>()
            + self
                .stages
                .iter()
                .map(|(s, ids)| {
                    std::mem::size_of::<Stage>()
                        + 2 * std::mem::size_of::<usize>()
                        + s.ranges.capacity() * std::mem::size_of::<Range>()
                        + ids.capacity() * std::mem::size_of::<Option<u64>>()
                })
                .sum::<usize>()
    }
}

/// Identity of the immutable raw descriptors actually encoded, not a later pool epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawVersion {
    pub generation: u64,
    pub table: (u64, u64),
    pub log: (u64, u64),
}

struct CachedSet<K> {
    key: K,
    offset: u64,
    evidence: Option<Arc<Stage>>,
    raw: Option<RawVersion>,
    /// Pool epoch sampled before ordinary resource encoding, never at publication.
    memory_generation: u64,
}

/// The actual descriptor-set cache and a separate active command view. At most seven cached and
/// seven active snapshots per queue, in addition to the charged history. Uncacheable writes must
/// not replace evidence belonging to a still-cached set, nor may compute clear the graphics cache.
pub struct SetCache<K> {
    cached: [Option<CachedSet<K>>; 7],
    active: [Option<Arc<Stage>>; 7],
    required: [bool; 7],
}

impl<K> Default for SetCache<K> {
    fn default() -> Self {
        Self {
            cached: std::array::from_fn(|_| None),
            active: Default::default(),
            required: [false; 7],
        }
    }
}

impl<K: PartialEq> SetCache<K> {
    pub fn begin(&mut self, stages: impl IntoIterator<Item = usize>) {
        self.active = Default::default();
        self.required = [false; 7];
        for stage in stages {
            self.required[stage] = true;
        }
    }

    pub fn hit(&self, stage: usize, key: K) -> Option<(u64, Option<Arc<Stage>>)> {
        self.hit_version(stage, key, None, 0)
    }

    pub fn hit_version(&self, stage: usize, key: K, raw: Option<RawVersion>,
        current_generation: u64) -> Option<(u64, Option<Arc<Stage>>)> {
        if raw.is_some_and(|v| v.generation != current_generation) { return None; }
        self.cached[stage]
            .as_ref()
            .filter(|c| c.key == key && c.raw == raw && c.memory_generation == current_generation)
            .map(|c| (c.offset, c.evidence.clone()))
    }

    /// Called only for a cacheable write. None invalidates a key that changed during encoding;
    /// an uncacheable write bypasses this method entirely, preserving the old cache entry.
    pub fn store(
        &mut self,
        stage: usize,
        key: Option<K>,
        offset: u64,
        evidence: Option<Arc<Stage>>,
    ) {
        self.store_version(stage, key, offset, evidence, None, 0, 0);
    }

    /// Publication uses both captured identities. Any pool change during ordinary
    /// encoding (including backing created by resolution) discards cacheability,
    /// even when raw is None or the raw pair was captured after that change.
    pub fn store_version(&mut self, stage: usize, key: Option<K>, offset: u64,
        evidence: Option<Arc<Stage>>, raw: Option<RawVersion>,
        memory_generation: u64, current_generation: u64) {
        let key = key.filter(|_| memory_generation == current_generation
            && raw.is_none_or(|v| v.generation == current_generation));
        self.cached[stage] = key.map(|key| CachedSet {
            key,
            offset,
            evidence,
            raw,
            memory_generation,
        });
    }

    pub fn select(
        &mut self,
        stage: usize,
        serial: u64,
        chunk: usize,
        offset: u64,
        shader: [u8; 8],
        evidence: Option<Arc<Stage>>,
    ) {
        self.active[stage] =
            evidence.filter(|e| e.matches(serial, chunk, offset, stage as u32, shader));
    }

    pub fn capture(&self, observe: impl Fn(&Range) -> Option<u64>) -> (usize, StageUses) {
        let mut missing = 0;
        let mut stages = Vec::new();
        for (required, active) in self.required.iter().zip(&self.active) {
            if !required {
                continue;
            }
            match active {
                Some(s) => stages.push((s.clone(), s.ranges.iter().map(&observe).collect())),
                None => missing += 1,
            }
        }
        (missing, stages)
    }
}

struct Selection<'a> {
    draws: Vec<&'a Draw>,
    retained: usize,
}

#[cfg(test)]
mod raw_cache_boundary_tests {
    use super::*;

    #[test]
    fn captured_pairs_cannot_be_relabelled_during_resolution_publication_or_hit() {
        // Exercise the very SetCache methods called by bind_sets. `capture` stands
        // for the immutable result returned by gmem::bindings, not a live epoch key.
        let capture = |generation, offset| RawVersion {
            generation, table: (100, offset), log: (100, offset + 262144),
        };
        for change_at in 0..5 {
            let mut cache = SetCache::<u64>::default();
            let mut epoch = 10;
            let old = capture(epoch, 0);
            cache.store_version(5, Some(7), 32, None, Some(old), epoch, epoch);
            assert!(cache.hit_version(5, 7, Some(old), epoch).is_some());
            if change_at == 0 { epoch += 1; } // before resource resolution
            // Force another encoding, as a changed ordinary binding would do.
            assert!(cache.hit_version(5, 8, Some(old), epoch).is_none());
            let encoding_epoch = epoch; // stage_set entry, before ordinary resolution
            if change_at == 1 { epoch += 1; } // an ordinary resolve adds backing
            let encoded = capture(epoch, 524288);
            if change_at == 2 { epoch += 1; } // after pair capture, before publication
            cache.store_version(5, Some(8), 64, None, Some(encoded), encoding_epoch, epoch);
            if change_at == 3 { epoch += 1; } // after publication
            if change_at == 4 { epoch += 1; } // just before lookup
            let hit = cache.hit_version(5, 8, Some(encoded), epoch);
            assert_eq!(hit.is_some(), change_at == 0);
            if change_at >= 1 {
                let replacement = capture(epoch, 1048576);
                // Even equal ordinary keys do not grant a hit for an unencoded pair.
                assert!(cache.hit_version(5, 8, Some(replacement), epoch).is_none());
                cache.store_version(5, Some(8), 96, None, Some(replacement), epoch, epoch);
                assert_eq!(cache.hit_version(5, 8, Some(replacement), epoch).unwrap().0, 96);
            }
            assert!(cache.hit_version(5, 8, Some(old), epoch).is_none());
        }
    }

    #[test]
    fn ordinary_buffer_without_raw_token_hits_only_its_captured_pool_epoch() {
        let mut cache = SetCache::<u64>::default();
        // Same module, guest addresses/ranges and binding-state key; no raw resource.
        cache.store_version(5, Some(7), 32, None, None, 10, 10);
        assert_eq!(cache.hit_version(5, 7, None, 10).unwrap().0, 32);
        assert!(cache.hit_version(5, 7, None, 11).is_none(),
            "pool replacement must invalidate ordinary UBO/SSBO descriptors too");
        cache.store_version(5, Some(7), 64, None, None, 11, 11);
        assert_eq!(cache.hit_version(5, 7, None, 11).unwrap().0, 64);
    }

    #[test]
    fn ordinary_encoding_mutation_cannot_publish_old_backing_as_new_epoch() {
        let mut cache = SetCache::<u64>::default();
        let mut epoch = 10;
        let encoding_epoch = epoch; // actual stage_set capture boundary
        let encoded_offset = 32; // descriptors resolved to the old backing
        epoch += 1; // pool creation/replacement before publication
        cache.store_version(5, Some(7), encoded_offset, None, None, encoding_epoch, epoch);
        assert!(cache.hit_version(5, 7, None, epoch).is_none());
        assert!(cache.hit_version(5, 7, None, encoding_epoch).is_none(),
            "an unstable encoding must not be published under either epoch");
        // The next stable encoding can cache normally with no forced refresh loop.
        cache.store_version(5, Some(7), 64, None, None, epoch, epoch);
        assert_eq!(cache.hit_version(5, 7, None, epoch).unwrap().0, 64);
    }

    #[test]
    fn raw_pair_and_ordinary_encoding_epochs_are_independent_admission_checks() {
        let mut cache = SetCache::<u64>::default();
        let old = RawVersion { generation: 10, table: (100, 0), log: (100, 262144) };
        let new = RawVersion { generation: 11, table: (100, 524288), log: (100, 786432) };
        // A current raw pair does not repair ordinary descriptors encoded before a pool change.
        cache.store_version(5, Some(7), 32, None, Some(new), 10, 11);
        assert!(cache.hit_version(5, 7, Some(new), 11).is_none());
        // A current ordinary epoch cannot relabel a previously captured raw pair (GAR-2).
        cache.store_version(5, Some(7), 64, None, Some(old), 11, 11);
        assert!(cache.hit_version(5, 7, Some(old), 11).is_none());
        assert!(cache.hit_version(5, 7, Some(new), 11).is_none());
        cache.store_version(5, Some(7), 96, None, Some(new), 11, 11);
        assert_eq!(cache.hit_version(5, 7, Some(new), 11).unwrap().0, 96);
        assert!(cache.hit_version(5, 7, Some(new), 12).is_none());
    }
}

impl Selection<'_> {
    fn omitted(&self) -> usize {
        self.retained - self.draws.len()
    }
}

#[derive(Debug, PartialEq, Eq)]
enum MarkerEvidence {
    Retained {
        shown: usize,
        omitted: usize,
    },
    /// No retained issued association: unissued, disabled/unobserved, or evicted. Not an empty set.
    Unavailable,
}

#[derive(Default)]
struct History {
    draws: VecDeque<Draw>,
    bytes: usize,
    evicted: u64,
}

impl History {
    fn push(&mut self, draw: Draw, max_draws: usize, max_bytes: usize) {
        let charge = draw.charge();
        while !self.draws.is_empty()
            && (self.draws.len() >= max_draws || self.bytes + charge > max_bytes)
        {
            self.bytes -= self.draws.pop_front().unwrap().charge();
            self.evicted += 1;
        }
        if max_draws == 0 || charge > max_bytes {
            self.evicted += 1;
            return;
        }
        self.bytes += charge;
        self.draws.push_back(draw);
    }

    #[cfg(test)]
    fn window(&self, lo: u64, hi: u64) -> impl Iterator<Item = &Draw> {
        self.draws
            .iter()
            .filter(move |d| d.checkpoint >= lo && d.checkpoint <= hi)
    }

    /// Preserve explicit markers first, then their setup associations, then other candidates in the
    /// numeric envelope. Allocation order is not pre/main execution order; do not subtract a lookback.
    fn select(&self, markers: &[u64], cap: usize) -> Selection<'_> {
        let lo = markers.iter().copied().filter(|&m| m != 0).min();
        let hi = markers.iter().copied().max().unwrap_or(0);
        let priority = |d: &Draw| {
            if markers.contains(&d.checkpoint) && d.checkpoint != 0 {
                0
            } else if markers.iter().any(|&m| d.associates(m)) {
                1
            } else if lo.is_some_and(|lo| d.checkpoint <= hi && d.setup_last >= lo) {
                2
            } else {
                3
            }
        };
        let mut result = Selection {
            draws: Vec::new(),
            retained: 0,
        };
        for rank in 0..3 {
            for draw in &self.draws {
                if priority(draw) == rank {
                    result.retained += 1;
                    if result.draws.len() < cap {
                        result.draws.push(draw);
                    }
                }
            }
        }
        result
    }

    fn marker_evidence(&self, marker: u64, selected: &Selection<'_>) -> MarkerEvidence {
        let retained = self.draws.iter().filter(|d| d.associates(marker)).count();
        if retained == 0 {
            return MarkerEvidence::Unavailable;
        }
        let shown = selected
            .draws
            .iter()
            .filter(|d| d.associates(marker))
            .count();
        MarkerEvidence::Retained {
            shown,
            omitted: retained - shown,
        }
    }
}

static HISTORY: Mutex<History> = Mutex::new(History {
    draws: VecDeque::new(),
    bytes: 0,
    evicted: 0,
});

pub fn record(draw: Draw) {
    HISTORY.lock().unwrap().push(draw, MAX_DRAWS, MAX_BYTES);
}

/// A checkpoint bounds work, not the faulting instruction. Only compare explicitly associated draws.
pub fn report(markers: &[u64], address: u64) {
    if !enabled() {
        return;
    }
    let h = HISTORY.lock().unwrap();
    let selected = h.select(markers, 64);
    crate::log::red(&format!(
        "    mesh binding evidence: explicit markers {markers:?}, {} history records, {} evicted; {} matching retained, {} shown, {} retained-but-omitted by output cap; descriptor-buffer path only; point address comparison (driver precision still applies), not instruction attribution",
        h.draws.len(), h.evicted, selected.retained, selected.draws.len(), selected.omitted()
    ));
    for &marker in markers {
        match h.marker_evidence(marker, &selected) {
            MarkerEvidence::Retained { shown, omitted } => crate::log::red(&format!(
                "      explicit marker #{marker}: {shown} associated issued draws shown, {omitted} retained-but-omitted by output cap"
            )),
            MarkerEvidence::Unavailable => crate::log::red(&format!(
                "      explicit marker #{marker}: issued mesh association unavailable (unissued, unobserved or evicted); not an empty binding set"
            )),
        }
    }
    for d in &selected.draws {
        crate::log::red(&format!(
            "      draw #{} setup through #{} submission {} cb {:#x} frame {} pool epoch {} missing stages {}",
            d.checkpoint, d.setup_last, d.serial, d.command_buffer, d.frame, d.pool_epoch, d.missing_stages
        ));
        let nearest = d
            .stages
            .iter()
            .flat_map(|(s, _)| &s.ranges)
            .filter_map(|r| match r.relation(address) {
                Relation::Outside(n) => Some(n),
                _ => None,
            })
            .min();
        for (s, ids) in &d.stages {
            crate::log::red(&format!(
                "        stage {} shader {} written in submission {} chunk {} offset {:#x} pool epoch {} omitted ranges {}",
                s.stage, crate::nvn::prog::key_hex(&s.shader), s.serial, s.chunk, s.offset,
                s.pool_epoch, s.omitted
            ));
            for (i, r) in s.ranges.iter().enumerate() {
                let relation = match r.relation(address) {
                    Relation::Contains => "CONTAINING".to_string(),
                    Relation::Outside(n) if Some(n) == nearest => {
                        format!("NEAREST-OUTSIDE gap {n:#x} (OOB candidate only)")
                    }
                    Relation::Outside(n) => format!("outside gap {n:#x}"),
                    Relation::Unknown => "unknown/invalid range".to_string(),
                };
                crate::log::red(&format!(
                    "          binding {} type {} buffer {:#x} offset {:#x} encoded {:#x}+{:#x}: {relation}; submission-owned {}; observed binding id write {:?} / issue {:?} / loss {:?}",
                    r.binding, r.kind, r.buffer, r.offset, r.address, r.size, r.owned,
                    r.binding_id, ids.get(i).copied().flatten(), super::binding_id(r.buffer, r.address)
                ));
            }
        }
    }
    if selected.retained == 0 {
        crate::log::red("      no mesh draw snapshot associated with these markers (disabled, unsupported, not issued, or evicted); no negative lifetime/OOB conclusion");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stage(serial: u64) -> Arc<Stage> {
        Arc::new(Stage {
            serial,
            chunk: 0,
            offset: 64,
            stage: 3,
            shader: [9; 8],
            pool_epoch: 5,
            ranges: vec![Range {
                binding: 2,
                kind: 7,
                buffer: 42,
                offset: 0,
                address: 0x1000,
                size: 0x100,
                binding_id: Some(17),
                owned: true,
            }],
            omitted: 0,
        })
    }

    fn draw(checkpoint: u64, s: Arc<Stage>) -> Draw {
        Draw {
            checkpoint,
            setup_last: checkpoint,
            serial: s.serial,
            command_buffer: 10,
            frame: 2,
            pool_epoch: 5,
            missing_stages: 0,
            stages: vec![(s, vec![Some(17)])],
        }
    }

    #[test]
    fn cached_snapshot_survives_writer_replacement_and_is_joined_to_exact_draw() {
        let mut h = History::default();
        let original = stage(7);
        h.push(draw(100, original.clone()), 10, MAX_BYTES);
        // Pass/upload checkpoint 101 must not steal draw 102's cache-hit association.
        h.push(draw(102, original.clone()), 10, MAX_BYTES);
        drop(original);
        // Same chunk/offset/handle reused by a NEW submission must not replace old evidence.
        h.push(draw(103, stage(8)), 10, MAX_BYTES);
        assert_eq!(h.window(101, 101).count(), 0);
        let hit = h.window(102, 102).next().unwrap();
        assert_eq!(hit.serial, 7);
        assert_eq!(hit.stages[0].0.ranges[0].binding_id, Some(17));
        assert!(Arc::ptr_eq(&h.draws[0].stages[0].0, &hit.stages[0].0));
        assert_eq!(h.window(103, 103).next().unwrap().serial, 8);
    }

    #[test]
    fn retention_evicts_whole_draws_by_count_and_bytes() {
        let mut h = History::default();
        let s = stage(1);
        let bytes = draw(1, s.clone()).charge();
        for n in 1..=3 {
            h.push(draw(n, s.clone()), 2, MAX_BYTES);
        }
        assert_eq!(h.window(1, 1).count(), 0);
        assert_eq!(h.draws.len(), 2);
        assert_eq!(h.evicted, 1);
        h.push(draw(4, s.clone()), 10, bytes);
        assert_eq!(h.draws.len(), 1);
        assert_eq!(h.draws[0].checkpoint, 4);
        assert_eq!(h.bytes, bytes);
        h.push(draw(5, s), 10, bytes - 1);
        assert!(h.draws.is_empty());
        assert_eq!(h.bytes, 0);
        assert_eq!(h.evicted, 5);
    }

    #[test]
    fn containing_is_not_nearest_outside_and_ranges_are_half_open() {
        let s = stage(1);
        let r = &s.ranges[0];
        assert_eq!(r.relation(0x1000), Relation::Contains);
        assert_eq!(r.relation(0x10ff), Relation::Contains);
        assert_eq!(r.relation(0x1100), Relation::Outside(0));
        assert_eq!(r.relation(0x1200), Relation::Outside(0x100));
        assert_eq!(r.relation(0xff0), Relation::Outside(0x10));
        let mut other = r.clone();
        other.address = 0x1080;
        other.size = 0x10;
        assert_eq!(r.relation(0x1090), Relation::Contains);
        assert_eq!(other.relation(0x1090), Relation::Outside(0));
        other.address = u64::MAX - 1;
        assert_eq!(other.relation(u64::MAX), Relation::Unknown);
        other.size = 0;
        assert_eq!(other.relation(u64::MAX), Relation::Unknown);
    }

    #[test]
    fn cache_identity_and_binding_generation_cannot_be_replaced_by_recycled_address() {
        let s = stage(7);
        assert!(s.matches(7, 0, 64, 3, [9; 8]));
        assert!(!s.matches(8, 0, 64, 3, [9; 8]));
        assert!(!s.matches(7, 1, 64, 3, [9; 8]));
        assert!(!s.matches(7, 0, 128, 3, [9; 8]));
        assert!(!s.matches(7, 0, 64, 4, [9; 8]));
        assert!(!s.matches(7, 0, 64, 3, [8; 8]));
        let mut d = draw(102, s);
        // Same VA/handle, different observed binding at issue; keep both, do not relabel the write.
        d.stages[0].1[0] = Some(18);
        let mut h = History::default();
        h.push(d, 10, MAX_BYTES);
        let kept = h.window(102, 102).next().unwrap();
        assert_eq!(kept.stages[0].0.ranges[0].binding_id, Some(17));
        assert_eq!(kept.stages[0].1[0], Some(18));
    }

    #[test]
    fn per_set_truncation_is_explicit_and_does_not_hide_retained_bindings() {
        let mut s = Arc::try_unwrap(stage(1)).unwrap();
        let r = s.ranges[0].clone();
        for _ in 0..MAX_RANGES + 3 {
            s.push(r.clone());
        }
        assert_eq!(s.ranges.len(), MAX_RANGES);
        assert_eq!(s.omitted, 4);
        assert_eq!(s.ranges[0].relation(0x1001), Relation::Contains);
    }

    #[test]
    fn cached_mesh_evidence_survives_compute_and_uncacheable_interludes() {
        // Use the same cache hit/store/begin/select/capture operations as bind_sets and mesh issue.
        let mut cache = SetCache::default();
        let mut a = Arc::try_unwrap(stage(7)).unwrap();
        a.stage = 1; // A fragment stage need not depend on each mesh draw's staged records.
        let a = Arc::new(a);
        let key_a = (7, 0, a.shader);
        cache.begin([1]);
        cache.store(1, Some(key_a), 64, Some(a.clone()));
        cache.select(1, 7, 0, 64, a.shader, Some(a.clone()));

        let mut compute = Arc::try_unwrap(stage(7)).unwrap();
        compute.stage = 5;
        compute.offset = 128;
        let compute = Arc::new(compute);
        cache.begin([5]);
        cache.store(5, Some(key_a), 128, Some(compute.clone()));
        cache.select(5, 7, 0, 128, compute.shader, Some(compute));
        assert_eq!(cache.capture(|_| Some(18)).1[0].0.stage, 5);

        cache.begin([1]);
        let (offset, evidence) = cache.hit(1, key_a).unwrap();
        cache.select(1, 7, 0, offset, a.shader, evidence);
        let (missing, active) = cache.capture(|_| Some(18));
        assert_eq!(missing, 0);
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].0.stage, 1);
        assert_eq!(active[0].0.ranges[0].address, 0x1000);
        assert_eq!(active[0].0.ranges[0].binding_id, Some(17));
        assert_eq!(active[0].1, [Some(18)]);

        let mut b = Arc::try_unwrap(stage(7)).unwrap();
        b.stage = 1;
        b.shader = [8; 8];
        b.offset = 192;
        b.ranges[0].address = 0x9000;
        cache.begin([1]);
        // Uncacheable B selects the freshly encoded set, bypassing store as production does.
        cache.select(1, 7, 0, 192, b.shader, Some(Arc::new(b)));
        assert_eq!(cache.capture(|_| Some(19)).1[0].0.ranges[0].address, 0x9000);
        cache.begin([1]);
        let (offset, evidence) = cache.hit(1, key_a).unwrap();
        cache.select(1, 7, 0, offset, a.shader, evidence);
        let (missing, active) = cache.capture(|_| Some(18));
        assert_eq!(missing, 0);
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].0.ranges[0].address, 0x1000);
        assert_eq!(active[0].0.ranges[0].binding_id, Some(17));
        assert_eq!(active[0].1, [Some(18)]);

        for (serial, chunk, shader) in [(8, 0, a.shader), (7, 1, a.shader), (7, 0, [8; 8])] {
            assert!(cache.hit(1, (serial, chunk, shader)).is_none());
            cache.begin([1]);
            cache.select(1, serial, chunk, 64, shader, Some(a.clone()));
            let (missing, active) = cache.capture(|_| Some(18));
            assert_eq!(missing, 1);
            assert!(active.is_empty());
        }
        cache.store(1, None, 64, Some(a)); // Key invalidated during a cacheable encoding.
        assert!(cache.hit(1, key_a).is_none());
    }

    #[test]
    fn loss_selection_keeps_draw_across_setup_markers_and_pre_main_reordering() {
        let mut h = History::default();
        let s = stage(7);
        let mut d = draw(100, s.clone());
        // Four upload markers 101..104 in pre, then pass marker 105 in main before mesh issue.
        d.setup_last = 105;
        h.push(d, MAX_DRAWS, MAX_BYTES);
        for markers in [[105, 105], [104, 100]] {
            let selected = h.select(&markers, 64);
            assert_eq!(selected.draws.len(), 1);
            assert_eq!(selected.draws[0].checkpoint, 100);
            assert_eq!(selected.omitted(), 0);
            for marker in markers {
                assert_eq!(
                    h.marker_evidence(marker, &selected),
                    MarkerEvidence::Retained {
                        shown: 1,
                        omitted: 0
                    }
                );
            }
        }

        let mut d = draw(200, s.clone());
        d.setup_last = 201; // One intervening setup marker also works.
        h.push(d, MAX_DRAWS, MAX_BYTES);
        assert_eq!(h.select(&[201], 64).draws[0].checkpoint, 200);
        let unissued = h.select(&[300], 64); // Skipped command: no issue record was inserted.
        assert!(unissued.draws.is_empty());
        assert_eq!(
            h.marker_evidence(300, &unissued),
            MarkerEvidence::Unavailable
        );

        h.push(draw(400, s.clone()), 1, MAX_BYTES); // Evict the genuine old issued associations.
        let evicted = h.select(&[105, 100], 64);
        assert!(evicted.draws.is_empty());
        assert!(h.evicted > 0);
        assert_eq!(
            h.marker_evidence(105, &evicted),
            MarkerEvidence::Unavailable
        );

        let mut h = History::default();
        for checkpoint in 1000..=1070 {
            h.push(draw(checkpoint, s.clone()), MAX_DRAWS, MAX_BYTES);
        }
        // Exact driver markers outrank the other retained candidates in their envelope.
        let selected = h.select(&[1000, 1070], 2);
        assert_eq!(
            selected
                .draws
                .iter()
                .map(|d| d.checkpoint)
                .collect::<Vec<_>>(),
            [1000, 1070]
        );
        assert_eq!(selected.retained, 71);
        assert_eq!(selected.omitted(), 69);
        assert_eq!(
            h.marker_evidence(1030, &selected),
            MarkerEvidence::Retained {
                shown: 0,
                omitted: 1
            }
        );
        let capped = h.select(&[1000, 1030, 1070], 2);
        assert_eq!(
            h.marker_evidence(1070, &capped),
            MarkerEvidence::Retained {
                shown: 0,
                omitted: 1
            }
        );
        let zero_cap = h.select(&[1070], 0);
        assert_eq!(zero_cap.omitted(), 1);
        assert_eq!(
            h.marker_evidence(1070, &zero_cap),
            MarkerEvidence::Retained {
                shown: 0,
                omitted: 1
            }
        );
        assert!(h.select(&[], 64).draws.is_empty());
        assert!(h.select(&[0], 64).draws.is_empty());
    }
}
